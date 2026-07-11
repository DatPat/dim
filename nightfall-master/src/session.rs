use crate::error::NightfallError;
use crate::profiles::ProfileContext;
use crate::profiles::StreamType;
use crate::profiles::TranscodingProfile;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
use std::time::Instant;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::task::JoinHandle;

use tokio_stream::wrappers::LinesStream;
use tokio_stream::StreamExt;

use tracing::debug;
use tracing::info;
use tracing::warn;

/// Represents how many chunks we encode before we require a timeout reset.
/// Basically if within MAX_CHUNKS_AHEAD we do not get a timeout reset we kill the stream.
/// This can be tuned
const MAX_CHUNKS_AHEAD: u32 = 15;

// FIXME: This lazy static should be removed in favour of adding a new stats field to a session and
// sharing it between two threads at max rather than per whole lib.
lazy_static::lazy_static! {
    /// This static contains stats about each stream. It is a Map of maps containing k/v pairs
    /// parsed from the ffmpeg stdout. Each Map is keyed by a session id.
    pub static ref STREAMING_SESSION: Arc<RwLock<HashMap<String, HashMap<String, String>>>> =
        Arc::new(RwLock::new(HashMap::new()));
}

pub struct Session {
    /// Id of a stream in the form of a UUID.
    pub id: String,
    /// Indicates whether this stream is currently being throttled or not.
    pub is_throttled: bool,
    /// A list of fallback transcoding profiles. Nightfall will start using profiles from here if
    /// the first profile fails.
    pub profile_chain: Vec<&'static dyn TranscodingProfile>,
    /// The current transcoding profile being used in this session.
    pub profile: &'static dyn TranscodingProfile,
    /// The profile context for this session. This struct contains important information like
    /// target bitrate and container.
    pub profile_ctx: ProfileContext,
    /// The exit status of the underlying ffmpeg process.
    pub exit_status: Option<ExitStatus>,
    pub real_segment: u32,
    /// How many chunks have we returned so far since init.mp4 was returned.
    pub chunks_since_init: u32,
    pub chunk_size: u32,

    has_started: bool,
    last_chunk: u32,
    hard_timeout: Instant,
    child_pid: Option<u32>,
    real_process: Option<Child>,

    _process: Option<JoinHandle<()>>,
}

impl Session {
    pub fn new(
        id: String,
        mut profile_chain: Vec<&'static dyn TranscodingProfile>,
        profile_ctx: ProfileContext,
    ) -> Self {
        let profile = profile_chain.pop().expect("Profile chain is empty.");

        Self {
            id,
            profile,
            profile_chain,
            real_segment: profile_ctx.output_ctx.start_num,
            chunk_size: profile_ctx.output_ctx.target_gop,
            profile_ctx,
            last_chunk: 0,
            _process: None,
            is_throttled: false,
            has_started: false,
            child_pid: None,
            real_process: None,
            hard_timeout: Instant::now() + Duration::from_secs(30 * 60),
            chunks_since_init: 0,
            exit_status: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), io::Error> {
        // make sure we actually have a path to write files to.
        self.has_started = true;
        self.is_throttled = false;

        info!(
            session = %self.id,
            profile = %self.profile.tag(),
            profile_name = %self.profile.name(),
            profile_type = ?self.profile.profile_type(),
            input_codec = %self.profile_ctx.input_ctx.codec,
            output_codec = %self.profile_ctx.output_ctx.codec,
            remaining_fallbacks = self.profile_chain.len(),
            "Starting ffmpeg with profile"
        );

        let args = self.profile.build(self.profile_ctx.clone()).unwrap();

        let _ = std::fs::create_dir_all(&self.profile_ctx.output_ctx.outdir);
        let log_file = format!(
            "{}/ffmpeg_{}.log",
            &self.profile_ctx.output_ctx.outdir,
            self.profile.tag()
        );

        let mut stderr = File::create(log_file)?;
        let _ = stderr.write(args.as_slice().join(" ").as_ref());
        let _ = stderr.write(b"\n");
        let _ = stderr.flush();

        let stderr: Stdio = stderr.into();

        let stdout: Stdio = if self.profile.stream_type() == StreamType::Subtitle {
            File::create(format!("{}/stream", &self.profile_ctx.output_ctx.outdir))?.into()
        } else {
            Stdio::piped()
        };

        let mut process = Command::new(self.profile_ctx.ffmpeg_bin.clone())
            .stdout(stdout)
            .stderr(stderr)
            .stdin(Stdio::null())
            .args(args.as_slice())
            .spawn()?;

        self.child_pid = process.id();

        debug!(pid = self.child_pid, ffmpeg = %self.profile_ctx.ffmpeg_bin, ?args, "Started ffmpeg");

        if !self.profile.is_stdio_stream() {
            if let Some(stdout) = process.stdout.take() {
                let stdout_parser_thread =
                    StdoutParser::new(self.id.clone(), stdout, self.child_pid.clone().unwrap());

                self._process = Some(tokio::spawn(stdout_parser_thread.handle()));
            }
        }
        self.real_process = Some(process);

        Ok(())
    }

    // NOTE: This will only work for RawVideo streams.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.real_process.as_mut().and_then(|x| x.stdout.take())
    }

    pub fn start_num(&self) -> u32 {
        self.profile_ctx.output_ctx.start_num
    }

    pub fn next_profile(&mut self) -> Option<&str> {
        self.profile = self.profile_chain.pop()?;
        Some(self.profile.tag())
    }

    /// Check whether the underlying ffmpeg process died unexpectedly and, if
    /// so, advance to the next profile in the chain. Must be called from every
    /// request path (not just init requests) or a mid-stream encoder crash
    /// wedges the session forever.
    pub fn check_death_and_fallback(&mut self) -> Result<(), NightfallError> {
        self.try_wait();

        let Some(status) = self.exit_status.take() else {
            return Ok(());
        };

        if status.success() {
            return Ok(());
        }

        let failed_tag = self.profile.tag().to_string();
        let stderr_tail = self.stderr().unwrap_or_default();
        warn!(
            session = %self.id,
            profile = %failed_tag,
            exit_status = ?status,
            "Profile failed, ffmpeg stderr (last 1000 bytes):\n{}",
            stderr_tail,
        );

        match self.next_profile().map(ToString::to_string) {
            Some(next_tag) => {
                warn!(
                    session = %self.id,
                    "Falling back from '{}' to '{}'",
                    failed_tag,
                    next_tag,
                );
                // Purge whatever partial segments the crashed profile left
                // behind so `is_chunk_done` can't serve them.
                self.clean_outdir();
                self.reset_to(self.start_num());
                Ok(())
            }
            None => Err(NightfallError::ProfileChainExhausted),
        }
    }

    pub async fn join(&mut self) {
        if let Some(ref mut x) = self.real_process {
            let _ = x.kill().await;
            // Bounded wait: a process stuck in uninterruptible IO (dying NFS
            // mount, failing disk) can survive SIGKILL indefinitely — an
            // unbounded wait here would wedge the whole streaming actor and
            // freeze playback for every session.
            let _ = tokio::time::timeout(Duration::from_secs(5), x.wait()).await;
            // This is an intentional kill — a non-zero status here must not be
            // mistaken for a profile failure by the fallback logic.
            self.exit_status = None;
        }
    }

    pub fn stderr(&mut self) -> Option<String> {
        let file = format!(
            "{}/ffmpeg_{}.log",
            &self.profile_ctx.output_ctx.outdir,
            self.profile.tag()
        );

        let mut buf = String::new();
        let _ = File::open(file).ok()?.read_to_string(&mut buf);

        if buf.len() <= 2000 {
            return Some(buf);
        }

        // Keep the first line (ffmpeg command) and last 1000 bytes (error)
        let first_line = buf.lines().next().unwrap_or("").to_string();
        let tail = buf.split_off(buf.len() - 1000);
        Some(format!("{}\n...\n{}", first_line, tail))
    }

    pub fn try_wait(&mut self) -> bool {
        if let Some(ref mut x) = self.real_process {
            if let Ok(Some(status)) = x.try_wait() {
                self.exit_status = Some(status);
                return true;
            }
            // NOTE: Do not clear `exit_status` here — a status captured by an
            // earlier call must survive until the fallback logic consumes it.
        }

        false
    }

    pub fn is_hard_timeout(&self) -> bool {
        Instant::now() > self.hard_timeout
    }

    pub fn set_timeout(&mut self) {
        self.hard_timeout = Instant::now();
    }

    pub fn delete_tmp(&self) {
        let _ = fs::remove_dir_all(&self.profile_ctx.output_ctx.outdir);
    }

    pub fn is_dead(&self) -> bool {
        if let Some(x) = self.child_pid {
            return crate::utils::is_process_effectively_dead(x);
        }

        true
    }

    pub fn pause(&mut self) {
        if let Some(x) = self.child_pid {
            if !self.is_throttled {
                crate::utils::pause_proc(x as i32);
                self.is_throttled = true;
            }
        }
    }

    pub fn cont(&mut self) {
        if let Some(x) = self.child_pid {
            if self.is_throttled {
                crate::utils::cont_proc(x as i32);
                self.is_throttled = false;
            }
        }
    }

    pub fn get_key(&self, k: &str) -> Option<String> {
        let session = STREAMING_SESSION.read().unwrap();
        session.get(&self.id)?.get(k).cloned()
    }

    pub fn current_chunk(&self) -> u32 {
        // Segments are cut on wall-clock time (`-force_key_frames
        // expr:gte(t,n_forced*gop)`), so derive the chunk from the output
        // timestamp rather than the frame counter — a frame count is only
        // convertible to a chunk index by assuming a fixed framerate, which
        // broke every non-24fps source. With `-copyts` the output timestamp is
        // absolute media time, so no `start_num` offset is needed.
        match self.profile.stream_type() {
            StreamType::Audio { .. } | StreamType::Video { .. } => {
                let out_secs = self
                    .get_key("out_time_us")
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(0)
                    / 1_000_000;

                ((out_secs / self.chunk_size as u64) as u32).max(self.last_chunk)
            }
            _ => 0,
        }
    }

    pub fn raw_speed(&self) -> f64 {
        self.get_key("speed")
            .map(|x| x.trim_end_matches('x').to_string())
            .and_then(|x| x.parse::<f64>().ok())
            .unwrap_or(1.0) // assume if the key is missing that our speed is 1.0
    }

    // returns how many chunks per second
    pub fn speed(&self) -> f64 {
        // Guard only against a zero/negative reported speed; clamping upwards
        // (the old `.max(20.0)`) made `eta_for` wildly optimistic for slow
        // software transcodes and prevented eta-based hard seeks.
        self.raw_speed().max(0.1) / self.chunk_size as f64
    }

    pub fn eta_for(&self, chunk: u32) -> Duration {
        let cps = self.speed();

        let current_chunk = self.current_chunk() as f64;
        let diff = (chunk as f64 - current_chunk).abs();

        Duration::from_secs((diff / cps).abs().ceil() as u64)
    }

    /// Method does some math magic to guess if a chunk has been fully written by ffmpeg yet
    /// only works when `ffmpeg` writes files to tmp then renames them.
    pub fn is_chunk_done(&self, chunk_num: u32) -> bool {
        Path::new(&format!(
            "{}/{}.m4s",
            &self.profile_ctx.output_ctx.outdir, chunk_num
        ))
        .is_file()
    }

    pub fn subtitle(&self, file: String) -> Option<String> {
        if !matches!(self.profile.stream_type(), StreamType::Subtitle) {
            return None;
        }

        let file = format!("{}/{}", &self.profile_ctx.output_ctx.outdir, file);
        let path = Path::new(&file);

        // NOTE: This will not check if the ffmpeg process is dead, thus this will return immediately
        if path.is_file() {
            return path.to_str().map(ToString::to_string);
        }

        None
    }

    pub fn is_timeout(&self) -> bool {
        self.current_chunk() > self.last_chunk + MAX_CHUNKS_AHEAD
    }

    pub fn reset_timeout(&mut self, last_requested: u32) {
        self.last_chunk = last_requested;
        self.hard_timeout = Instant::now() + Duration::from_secs(30 * 60);
    }

    pub fn chunk_to_path(&self, chunk_num: u32) -> String {
        format!("{}/{}.m4s", self.profile_ctx.output_ctx.outdir, chunk_num)
    }

    pub fn init_seg(&self) -> String {
        format!(
            "{}/{}_init.mp4",
            self.profile_ctx.output_ctx.outdir,
            self.start_num()
        )
    }

    pub fn custom_init_seg(&self, start_num: u32) -> String {
        format!(
            "{}/{}_init.mp4",
            self.profile_ctx.output_ctx.outdir,
            start_num
        )
    }

    pub fn has_started(&self) -> bool {
        self.has_started
    }

    pub fn reset_to(&mut self, chunk: u32) {
        self.profile_ctx.output_ctx.start_num = chunk;
        self._process = None;
        self.last_chunk = chunk;
        self.has_started = false;
        self.is_throttled = true;
        self.real_segment = chunk;
        self.child_pid = None;
        // Drop the handle to the previous (killed or dead) process so a later
        // `join()` can't resurrect its exit status as a phantom profile failure.
        self.real_process = None;
    }

    /// Delete the media artifacts (segments, init fragments, playlists) the
    /// current outdir holds. Used when falling back to another profile so
    /// `is_chunk_done` can't pick up a crashed profile's partial output.
    pub fn clean_outdir(&self) {
        if let Ok(entries) = fs::read_dir(&self.profile_ctx.output_ctx.outdir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".m4s") || name.ends_with(".mp4") || name.ends_with(".m3u8") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("start_number", &self.profile_ctx.output_ctx.start_num)
            .field("last_chunk", &self.last_chunk)
            .finish()
    }
}

struct StdoutParser {
    id: String,
    process_stdout: ChildStdout,
    pid: u32,
}

impl StdoutParser {
    fn new(id: String, process_stdout: ChildStdout, pid: u32) -> Self {
        Self {
            id,
            process_stdout,
            pid,
        }
    }

    async fn handle(self) {
        let mut stdio = LinesStream::new(BufReader::new(self.process_stdout).lines());
        let mut map: HashMap<String, String> = HashMap::new();

        let interval = tokio::time::interval(Duration::from_millis(100));
        tokio::pin!(interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if crate::utils::is_process_effectively_dead(self.pid) {
                        break;
                    }
                },

                Some(Ok(v)) = stdio.next() => {
                    // `-progress` output is `key=value` lines; skip anything else
                    // instead of panicking the parser task on a stray line.
                    let Some((key, value)) = v.split_once('=') else {
                        continue;
                    };

                    map.insert(key.trim().into(), value.trim().into());

                    {
                        let mut lock = STREAMING_SESSION.write().unwrap();
                        let _ = lock.insert(self.id.clone(), map.clone());
                    }

                    continue;
                }
            }
        }

        let mut lock = STREAMING_SESSION.write().unwrap();
        let _ = lock.remove(&self.id);
    }
}
