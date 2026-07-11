use std::convert::TryFrom;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

use crate::NightfallError;
use crate::Result;

use tokio::task::spawn_blocking;

use tracing::debug;

// ---------------------------------------------------------------------------
// Binary helpers — avoid mp4-rust serialization bugs.
// ---------------------------------------------------------------------------

fn read_u32_be(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

fn read_u64_be(data: &[u8]) -> u64 {
    u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

/// Read a box's total size and header size at `pos`, honoring the 64-bit
/// `largesize` (size == 1) and to-end-of-data (size == 0) encodings. Returns
/// `None` on a truncated or malformed header.
fn box_size_at(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    if pos + 8 > data.len() {
        return None;
    }

    match read_u32_be(&data[pos..]) as usize {
        0 => Some((data.len() - pos, 8)),
        1 => {
            if pos + 16 > data.len() {
                return None;
            }
            let size = usize::try_from(read_u64_be(&data[pos + 8..])).ok()?;
            (size >= 16).then_some((size, 16))
        }
        s if s < 8 => None,
        s => Some((s, 8)),
    }
}

/// Patchable field offsets found within a raw segment byte range.
struct SegmentPatchInfo {
    /// earliest_presentation_time from sidx box (if present).
    sidx_ept: Option<u64>,
    /// Byte offset of mfhd.sequence_number within the segment group.
    mfhd_seq_offset: Option<usize>,
    /// Byte offset of tfdt.base_media_decode_time within the segment group.
    tfdt_bmd_offset: Option<usize>,
    /// Version of the tfdt box (0 → 4-byte time, ≥1 → 8-byte time).
    tfdt_version: u8,
}

/// Scan a raw segment byte range for patchable offsets without deserializing
/// through mp4-rust.  Descends into `moof`/`traf` container boxes.
fn scan_segment_patch_info(data: &[u8]) -> SegmentPatchInfo {
    let mut info = SegmentPatchInfo {
        sidx_ept: None,
        mfhd_seq_offset: None,
        tfdt_bmd_offset: None,
        tfdt_version: 0,
    };

    let len = data.len();
    let mut pos = 0usize;

    while pos + 8 <= len {
        let Some((box_size, header_size)) = box_size_at(data, pos) else {
            break;
        };
        let box_type = &data[pos + 4..pos + 8];

        // Container boxes — descend into children (skip just the header).
        if box_type == b"moof" || box_type == b"traf" {
            pos += header_size;
            continue;
        }

        if pos + box_size > len {
            break;
        }

        if box_type == b"sidx" {
            // sidx: header(8) + version(1)+flags(3) + ref_id(4) + timescale(4)
            //       + EPT (4 or 8 bytes depending on version)
            let version = data[pos + 8];
            if version == 0 && box_size >= 28 {
                info.sidx_ept = Some(read_u32_be(&data[pos + 20..]) as u64);
            } else if version >= 1 && box_size >= 32 {
                info.sidx_ept = Some(read_u64_be(&data[pos + 20..]));
            }
        } else if box_type == b"mfhd" && box_size == 16 {
            // mfhd: header(8) + version_flags(4) + sequence_number(4)
            info.mfhd_seq_offset = Some(pos + 12);
        } else if box_type == b"tfdt" {
            // tfdt: header(8) + version(1)+flags(3) + base_media_decode_time(4 or 8)
            let version = data[pos + 8];
            if version == 0 && box_size >= 16 {
                info.tfdt_version = 0;
                info.tfdt_bmd_offset = Some(pos + 12);
            } else if version >= 1 && box_size >= 20 {
                info.tfdt_version = version;
                info.tfdt_bmd_offset = Some(pos + 12);
            }
        }

        pos += box_size;
    }

    info
}

/// Create a `styp` box by cloning the `ftyp` box from the init data and
/// changing the type field.  Falls back to a minimal default if no ftyp is
/// found.
fn make_styp_from_init(init_data: &[u8]) -> Vec<u8> {
    let len = init_data.len();
    let mut pos = 0usize;
    while pos + 8 <= len {
        let Some((box_size, _)) = box_size_at(init_data, pos) else {
            break;
        };
        if pos + box_size > len {
            break;
        }
        if &init_data[pos + 4..pos + 8] == b"ftyp" {
            let mut styp = init_data[pos..pos + box_size].to_vec();
            styp[4..8].copy_from_slice(b"styp");
            return styp;
        }
        pos += box_size;
    }
    // Fallback: minimal styp { major_brand: "isom", minor_version: 0, compat: ["isom"] }
    let mut styp = Vec::with_capacity(20);
    styp.extend_from_slice(&20u32.to_be_bytes());
    styp.extend_from_slice(b"styp");
    styp.extend_from_slice(b"isom");
    styp.extend_from_slice(&0u32.to_be_bytes());
    styp.extend_from_slice(b"isom");
    styp
}

/// Read an init segment, move embedded audio-visual segments into
/// `segment_path`, and rewrite the init file without them.
///
/// This is a binary-patching replacement for the original implementation that
/// went through mp4-rust's buggy read→modify→write round-trip.  Only the
/// `mfhd.sequence_number` and `tfdt.base_media_decode_time` fields are
/// modified; every other byte is copied verbatim.
///
/// # Arguments
/// * `init` – Path to the initialization segment.
/// * `segment_path` – Path to the output segment file.
/// * `seq` – Starting sequence number.
///
/// # Returns
/// The next sequence number (`seq` + number of segments written).
pub async fn patch_init_segment(
    init: impl AsRef<Path> + Send + 'static,
    segment_path: impl AsRef<Path> + Send + 'static,
    mut seq: u32,
) -> Result<u32> {
    spawn_blocking(move || {
        let data = std::fs::read(&init)?;
        let len = data.len();

        // Phase 1: Find where the init-only data ends.  The init section
        // consists of `ftyp`, `moov`, and any other non-segment boxes
        // (e.g. `free`).  The first segment-related box (`sidx`, `moof`,
        // `mdat`, or `styp`) marks the start of embedded segment data.
        let mut pos = 0usize;
        let mut init_end = 0usize;

        while pos + 8 <= len {
            let Some((box_size, _)) = box_size_at(&data, pos) else {
                break;
            };
            let box_type = &data[pos + 4..pos + 8];

            if pos + box_size > len {
                break;
            }

            if box_type == b"sidx"
                || box_type == b"moof"
                || box_type == b"mdat"
                || box_type == b"styp"
            {
                break;
            }

            init_end = pos + box_size;
            pos += box_size;
        }

        let segment_bytes = &data[init_end..];
        if segment_bytes.is_empty() {
            return Ok(seq);
        }

        // Phase 2: Generate a styp box from the init's ftyp.
        let styp = make_styp_from_init(&data[..init_end]);

        // Phase 3: Walk the segment bytes, grouping by `mdat` (each `mdat`
        // terminates one segment).  Copy each group verbatim with binary
        // patches applied to mfhd and tfdt.
        let mut out = File::create(&segment_path)?;
        let seg_len = segment_bytes.len();
        let mut seg_pos = 0usize;
        let mut group_start = 0usize;
        let mut segments_written = 0u32;

        while seg_pos + 8 <= seg_len {
            let Some((box_size, _)) = box_size_at(segment_bytes, seg_pos) else {
                break;
            };
            let box_type = &segment_bytes[seg_pos + 4..seg_pos + 8];

            if seg_pos + box_size > seg_len {
                break;
            }

            if box_type == b"mdat" {
                let group_end = seg_pos + box_size;
                let group = &segment_bytes[group_start..group_end];

                // Scan for patchable offsets.
                let info = scan_segment_patch_info(group);

                // Copy to a mutable buffer and apply binary patches.
                let mut buf = group.to_vec();

                if let Some(offset) = info.mfhd_seq_offset {
                    buf[offset..offset + 4].copy_from_slice(&seq.to_be_bytes());
                }

                if let (Some(ept), Some(offset)) = (info.sidx_ept, info.tfdt_bmd_offset) {
                    if info.tfdt_version == 0 {
                        buf[offset..offset + 4].copy_from_slice(&(ept as u32).to_be_bytes());
                    } else {
                        buf[offset..offset + 8].copy_from_slice(&ept.to_be_bytes());
                    }
                }

                // Prepend a styp box if the group doesn't already have one.
                let has_styp = group.len() >= 8 && &group[4..8] == b"styp";
                if !has_styp {
                    out.write_all(&styp)?;
                }
                out.write_all(&buf)?;

                seq += 1;
                segments_written += 1;
                group_start = group_end;
            }

            seg_pos += box_size;
        }

        debug!(
            segments = segments_written,
            "Patched init segment (binary), moved embedded segments to chunk file."
        );

        // Phase 4: Rewrite the init file with only the init-only portion
        // (ftyp + moov), removing the embedded segments.
        std::fs::write(&init, &data[..init_end])?;

        Ok(seq)
    })
    .await
    .map_err(|e| NightfallError::IoError(format!("patch_init_segment task panicked: {e}")))?
}
