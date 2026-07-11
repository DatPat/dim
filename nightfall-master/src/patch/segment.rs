use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use crate::NightfallError;
use crate::Result;

use tokio::task::spawn_blocking;

use mp4::mp4box::*;
use tracing::debug;

/// Struct represents an individual segment from a stream.
///
/// NOTE: This is only used as the payload of `NightfallError::PartialSegment`.
/// Never serialize it back through mp4-rust's `write_box` — the fork has
/// multiple write bugs for moof/traf/trun/tfhd/tfdt; all patching is done
/// binary in-place instead.
#[derive(Clone, Default, Debug)]
pub struct Segment {
    /// styp box is needed if the segment is written to a separate file.
    /// in our case we just clone it from the parent init segment.
    pub styp: Option<FtypBox>,
    /// segment index box contains the index of the segment.
    pub sidx: Option<SidxBox>,
    /// Moof box contains metadata about the segment like the PTS and DTS.
    pub moof: Option<MoofBox>,
    /// Contains audio-visual data.
    pub mdat: Option<MdatBox>,
}

impl Segment {
    pub fn from_reader(mut reader: impl BufRead + Seek, size: u64) -> Result<(Self, u64)> {
        let start = reader.seek(SeekFrom::Current(0))?;

        let mut current = start;
        let mut segment = Self::default();

        while current < size {
            let header = BoxHeader::read(&mut reader)?;
            let BoxHeader { name, size: s } = header;

            match name {
                BoxType::SidxBox => {
                    segment.sidx = Some(SidxBox::read_box(&mut reader, s)?);
                }
                BoxType::MoofBox => {
                    segment.moof = Some(MoofBox::read_box(&mut reader, s)?);
                }
                BoxType::MdatBox => {
                    segment.mdat = Some(MdatBox::read_box(&mut reader, s)?);

                    // Since mdat would be the last box in the segment, we just return the segment
                    // here as well as the leftover bytes.
                    let leftover_bytes = reader.seek(SeekFrom::Current(0))?;
                    return Ok((segment, leftover_bytes));
                }
                BoxType::StypBox => {
                    let mut styp = FtypBox::read_box(&mut reader, s)?;
                    styp.box_type = BoxType::StypBox;
                    segment.styp = Some(styp);
                }
                b => {
                    debug!(box_type = %b, "Got a weird box type.");
                    skip_box(&mut reader, s)?;
                }
            }

            current = reader.seek(SeekFrom::Current(0))?;
        }

        // NOTE: In some cases, we could get here without a complete segment existing.
        Ok((segment, size))
    }
}

/// Read a box header at the reader's current position and return the box's
/// total size, honoring the 64-bit `largesize` (size == 1) and to-end-of-file
/// (size == 0) encodings. The reader is left just past the header. Returns
/// `None` on a truncated or malformed header.
fn read_box_size(
    reader: &mut (impl Read + Seek),
    pos: u64,
    file_size: u64,
) -> Option<(u64, [u8; 4])> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header).ok()?;

    let box_type = [header[4], header[5], header[6], header[7]];
    let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;

    let box_size = match size32 {
        0 => file_size.checked_sub(pos)?,
        1 => {
            let mut large = [0u8; 8];
            reader.read_exact(&mut large).ok()?;
            let size = u64::from_be_bytes(large);
            if size < 16 {
                return None;
            }
            size
        }
        s if s < 8 => return None,
        s => s,
    };

    Some((box_size, box_type))
}

/// Scan the file for mfhd boxes and return the byte offset of each
/// mfhd.sequence_number field (4 bytes, big-endian u32).
///
/// The mfhd box layout is: size(4) + "mfhd"(4) + version_flags(4) + sequence_number(4).
/// We return the offset of the sequence_number field.
fn find_mfhd_offsets(reader: &mut (impl Read + Seek), file_size: u64) -> Result<Vec<u64>> {
    let mut offsets = Vec::new();
    let mut pos = reader.seek(SeekFrom::Start(0))?;

    while pos + 8 <= file_size {
        let Some((box_size, box_type)) = read_box_size(reader, pos, file_size) else {
            break;
        };

        if &box_type == b"moof" || &box_type == b"traf" {
            // Container boxes — descend into children (skip just the 8-byte header).
            pos = reader.seek(SeekFrom::Start(pos + 8))?;
            continue;
        }

        if &box_type == b"mfhd" && box_size == 16 {
            // mfhd: version_flags(4) + sequence_number(4)
            // The sequence_number is at offset pos + 8 (header) + 4 (version_flags) = pos + 12
            offsets.push(pos + 12);
        }

        pos = reader.seek(SeekFrom::Start(pos + box_size))?;
    }

    Ok(offsets)
}

/// Check whether the segment is an empty (partial) segment that only contains a styp box.
fn is_empty_segment(reader: &mut (impl Read + Seek), file_size: u64) -> Result<bool> {
    reader.seek(SeekFrom::Start(0))?;

    let mut has_styp = false;
    let mut has_moof = false;
    let mut has_mdat = false;
    let mut pos = 0u64;

    while pos + 8 <= file_size {
        let Some((box_size, box_type)) = read_box_size(reader, pos, file_size) else {
            break;
        };

        if &box_type == b"styp" {
            has_styp = true;
        } else if &box_type == b"moof" {
            has_moof = true;
        } else if &box_type == b"mdat" {
            has_mdat = true;
        }

        pos = reader.seek(SeekFrom::Start(pos + box_size))?;
    }

    Ok(has_styp && !has_moof && !has_mdat)
}

/// Patch a segment file's mfhd sequence numbers in-place without re-serializing.
///
/// This avoids bugs in the mp4 crate's write path that corrupt segments during
/// round-trip serialization. Only the 4-byte sequence_number field in each mfhd
/// box is modified; everything else is left untouched.
///
/// # Arguments
/// * `file` - target input/output file.
/// * `seq` - starting sequence number.
///
/// # Returns
/// The next sequence number (seq + number of segments patched).
pub async fn patch_segment(file: impl AsRef<Path> + Send + 'static, mut seq: u32) -> Result<u32> {
    spawn_blocking(move || {
        let file_size = std::fs::metadata(&file)?.len();

        // First check if this is a partial/empty segment (needs full rewrite via
        // patch_init_segment).
        {
            let f = File::open(&file)?;
            let mut reader = BufReader::new(f);
            if is_empty_segment(&mut reader, file_size)? {
                // Parse the segment fully so we can return it in the error for
                // patch_init_segment to use.
                reader.seek(SeekFrom::Start(0))?;
                let (segment, _) = Segment::from_reader(&mut reader, file_size)?;
                return Err(NightfallError::PartialSegment(segment));
            }
        }

        // Patch mfhd sequence numbers in-place.
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file)?;
        let offsets = find_mfhd_offsets(&mut f, file_size)?;

        for offset in offsets {
            f.seek(SeekFrom::Start(offset))?;
            f.write_all(&seq.to_be_bytes())?;
            seq += 1;
        }

        Ok(seq)
    })
    .await
    .map_err(|e| NightfallError::IoError(format!("patch_segment task panicked: {e}")))?
}
