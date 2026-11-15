use super::ProfileContext;
use super::ProfileType;
use super::StreamType;
use super::TranscodingProfile;

use crate::error::NightfallError;

/// Audio transcode target. One profile instance is registered per supported
/// output codec; `supports()` matches on `output_ctx.codec == tag`.
#[derive(Debug)]
pub struct AudioTranscodeProfile {
    /// Codec tag used in `OutputCtx::codec` (aac, opus, flac, ac3, eac3).
    tag: &'static str,
    /// ffmpeg encoder name.
    encoder: &'static str,
    name: &'static str,
}

impl AudioTranscodeProfile {
    pub const AAC: Self = Self { tag: "aac", encoder: "aac", name: "AacTranscodeProfile" };
    pub const OPUS: Self = Self { tag: "opus", encoder: "libopus", name: "OpusTranscodeProfile" };
    pub const FLAC: Self = Self { tag: "flac", encoder: "flac", name: "FlacTranscodeProfile" };
    pub const AC3: Self = Self { tag: "ac3", encoder: "ac3", name: "Ac3TranscodeProfile" };
    pub const EAC3: Self = Self { tag: "eac3", encoder: "eac3", name: "Eac3TranscodeProfile" };

    pub const ALL: [Self; 5] = [Self::AAC, Self::OPUS, Self::FLAC, Self::AC3, Self::EAC3];

    /// Bitrate argument for this encoder, or `None` for lossless output.
    /// `requested` is usually the *source* bitrate, which can be far outside
    /// what lossy encoders accept (e.g. a 4 Mb/s TrueHD track).
    fn bitrate(&self, requested: Option<u64>, out_ch: u64) -> Option<u64> {
        let requested = requested.unwrap_or(120_000);
        match self.tag {
            // Unchanged legacy behaviour; the aac encoder clamps on its own.
            "aac" => Some(requested),
            // libopus hard-errors above 256 kb/s per channel.
            "opus" => Some(requested.clamp(64_000, 256_000 * out_ch.max(1))),
            // Source bitrates (often multi-Mb/s lossless) mean nothing here;
            // 448 kb/s is a standard AC-3 table rate and ample for stereo/5.1.
            "ac3" | "eac3" => Some(448_000),
            "flac" => None,
            _ => Some(requested),
        }
    }
}

impl TranscodingProfile for AudioTranscodeProfile {
    fn profile_type(&self) -> ProfileType {
        ProfileType::Transcode
    }

    fn stream_type(&self) -> StreamType {
        StreamType::Audio
    }

    fn name(&self) -> &str {
        self.name
    }

    fn build(&self, ctx: ProfileContext) -> Option<Vec<String>> {
        let start_num = ctx.output_ctx.start_num.to_string();
        let stream = format!("0:{}", ctx.input_ctx.stream);
        let init_seg = format!("{}_init.mp4", &start_num);
        let seg_name = format!("{}/%d.m4s", ctx.output_ctx.outdir);
        let outdir = format!("{}/playlist.m3u8", ctx.output_ctx.outdir);

        // NOTE: might need flags -fflages +genpts if seeking breaks.
        let mut args = vec![
            "-y".into(),
            "-ss".into(),
            (ctx.output_ctx.start_num * ctx.output_ctx.target_gop).to_string(),
            "-i".into(),
            ctx.file,
            "-copyts".into(),
            "-map".into(),
            stream,
            "-c:0".into(),
            self.encoder.into(),
        ];

        let in_ch = ctx.input_ctx.audio_channels;
        let out_ch = ctx.output_ctx.audio_channels;

        if in_ch > 2 && out_ch == 2 {
            // Tuned surround → stereo downmix.
            args.append(&mut vec![
                "-af".into(),
                "pan=stereo|FL=0.5*FC+0.707*FL+0.707*BL+0.5*LFE|FR=0.5*FC+0.707*FR+0.707*BR+0.5*LFE".into(),
            ]);
        } else if in_ch != out_ch {
            // Any other channel-count change (e.g. multi-channel output, mono
            // upmix): let ffmpeg pick the mixing matrix. The old code forced
            // the stereo pan filter here, which made multi-channel output
            // impossible and "downmixed" even when upmixing was requested.
            args.append(&mut vec!["-ac".into(), out_ch.to_string()]);
        }

        if let Some(ab) = self.bitrate(ctx.output_ctx.bitrate, out_ch) {
            args.push("-ab".into());
            args.push(ab.to_string());
        }

        args.append(&mut vec![
            "-start_at_zero".into(),
            "-fps_mode".into(),
            "auto".into(),
            "-avoid_negative_ts".into(),
            "make_non_negative".into(),
        ]);

        args.append(&mut vec![
            "-f".into(),
            "hls".into(),
            "-start_number".into(),
            start_num,
        ]);

        // needed so that in progress segments are named `tmp` and then renamed after the data is
        // on disk.
        // This in theory practically prevents the web server from returning a segment that is
        // in progress.
        args.append(&mut vec![
            "-hls_flags".into(),
            "temp_file".into(),
            "-max_delay".into(),
            "5000000".into(),
        ]);

        // these args are needed if we start a new stream in the middle of a old one, such as when
        // seeking. These args will reset the base decode ts to equal the earliest presentation
        // timestamp.
        if ctx.output_ctx.start_num > 0 {
            args.append(&mut vec![
                "-hls_segment_options".into(),
                "movflags=frag_custom+dash+delay_moov+frag_discont".into(),
            ]);
        } else {
            args.append(&mut vec![
                "-hls_segment_options".into(),
                "movflags=frag_custom+dash+delay_moov".into(),
            ]);
        }

        // args needed so we can distinguish between init fragments for new streams.
        // Basically on the web seeking works by reloading the entire video because of
        // discontinuity issues that browsers seem to not ignore like mpv.
        args.append(&mut vec!["-hls_fmp4_init_filename".into(), init_seg]);

        args.append(&mut vec![
            "-hls_time".into(),
            ctx.output_ctx.target_gop.to_string(),
            "-force_key_frames".into(),
            format!("expr:gte(t,n_forced*{})", ctx.output_ctx.target_gop),
        ]);

        args.append(&mut vec!["-hls_segment_type".into(), "1".into()]);
        args.append(&mut vec![
            "-loglevel".into(),
            "info".into(),
            "-progress".into(),
            "pipe:1".into(),
        ]);
        args.append(&mut vec!["-hls_segment_filename".into(), seg_name]);
        args.append(&mut vec![outdir]);

        Some(args)
    }

    fn supports(&self, ctx: &ProfileContext) -> Result<(), NightfallError> {
        if ctx.output_ctx.codec == self.tag {
            return Ok(());
        }

        Err(NightfallError::ProfileNotSupported(
            "Profile not supported.".into(),
        ))
    }

    fn tag(&self) -> &str {
        self.tag
    }
}
