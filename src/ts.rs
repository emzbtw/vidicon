//! Cutting the end off a recording (SPEC 9.4). mpv writes MPEG-TS: 188-byte
//! packets, each audio or video frame starting in one that carries its PTS.
//! The file is read from the end back: the part to cut is its last seconds,
//! and the file may hold hours.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

const PACKET: u64 = 188;
/// Packets read at a time.
const CHUNK: u64 = 4096;
/// PTS ticks per second.
const HZ: f64 = 90_000.0;
/// How far below the cut to read on: frames are muxed out of PTS order
/// (B-frames, audio against video), by well under this.
const MARGIN: i64 = 5 * 90_000;

/// Cuts the last `tail` off `file`: at the first packet starting a frame
/// timed at or after `tail` before the last one. A file no longer than
/// that, or with no timestamps, is left as it is.
pub fn cut_tail(file: &Path, tail: Duration) -> io::Result<()> {
    let mut f = OpenOptions::new().read(true).write(true).open(file)?;
    let tail = (tail.as_secs_f64() * HZ) as i64;
    if tail <= 0 {
        return Ok(());
    }
    // Frame starts as (position, PTS), found back to front.
    let mut starts: Vec<(u64, i64)> = Vec::new();
    let (mut last, mut first) = (i64::MIN, i64::MAX);
    let mut buf = vec![0; (CHUNK * PACKET) as usize];
    let mut end = f.metadata()?.len() / PACKET * PACKET;
    while end > 0 && !(last > i64::MIN && first < last - tail - MARGIN) {
        let begin = end.saturating_sub(CHUNK * PACKET);
        let chunk = &mut buf[..(end - begin) as usize];
        f.seek(SeekFrom::Start(begin))?;
        f.read_exact(chunk)?;
        for (i, packet) in chunk
            .as_chunks::<{ PACKET as usize }>()
            .0
            .iter()
            .enumerate()
            .rev()
        {
            if let Some(pts) = frame_pts(packet) {
                starts.push((begin + i as u64 * PACKET, pts));
                (last, first) = (last.max(pts), first.min(pts));
            }
        }
        end = begin;
    }
    let cut_at = last.saturating_sub(tail);
    if starts.is_empty() || first >= cut_at {
        return Ok(());
    }
    let cut = starts
        .iter()
        .filter(|&&(_, pts)| pts >= cut_at)
        .map(|&(pos, _)| pos)
        .min();
    match cut {
        Some(cut) => f.set_len(cut),
        None => Ok(()),
    }
}

/// The PTS of the frame a packet starts, if it starts one (a PES header
/// with a PTS at the start of its payload).
pub(crate) fn frame_pts(p: &[u8]) -> Option<i64> {
    // Sync byte, and payload_unit_start_indicator.
    if p[0] != 0x47 || p[1] & 0x40 == 0 {
        return None;
    }
    let payload = match (p[3] >> 4) & 3 {
        1 => &p[4..],
        3 => p.get(5 + p[4] as usize..)?,
        _ => return None,
    };
    if payload.len() < 14 || payload[..3] != [0, 0, 1] || payload[7] & 0x80 == 0 {
        return None;
    }
    let b: Vec<i64> = payload[9..14].iter().map(|&b| i64::from(b)).collect();
    Some((b[0] >> 1 & 7) << 30 | b[1] << 22 | (b[2] >> 1) << 15 | b[3] << 7 | b[4] >> 1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A packet on `pid`: a frame start timed `pts`, or its continuation.
    fn packet(pid: u16, pts: Option<i64>) -> Vec<u8> {
        let mut p = vec![0x47, (pid >> 8) as u8 & 0x1f, pid as u8, 0x10];
        if let Some(t) = pts {
            p[1] |= 0x40;
            p.extend([0, 0, 1, 0xe0, 0, 0, 0x80, 0x80, 5]);
            p.extend([
                0x21 | (t >> 29 & 0x0e) as u8,
                (t >> 22) as u8,
                (t >> 14 & 0xfe) as u8 | 1,
                (t >> 7) as u8,
                (t << 1 & 0xfe) as u8 | 1,
            ]);
        }
        p.resize(PACKET as usize, 0xff);
        p
    }

    /// 30 s of 25 fps video from 1 s, ten packets a frame, with an audio
    /// frame each 4 video frames: more than one [`CHUNK`].
    pub(crate) fn recording() -> (tempfile::NamedTempFile, Vec<u64>) {
        let mut data = Vec::new();
        let mut video_at = Vec::new();
        for i in 0..750 {
            video_at.push(data.len() as u64);
            data.extend(packet(0x100, Some(90_000 + i * 3600)));
            for _ in 0..9 {
                data.extend(packet(0x100, None));
            }
            if i % 4 == 0 {
                data.extend(packet(0x101, Some(90_000 + i * 3600 - 1800)));
            }
        }
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), data).unwrap();
        (file, video_at)
    }

    #[test]
    fn pts_is_read_from_frame_starts_only() {
        assert_eq!(
            frame_pts(&packet(0x100, Some(8_589_934_591))),
            Some(8_589_934_591)
        );
        assert_eq!(frame_pts(&packet(0x100, Some(123_456))), Some(123_456));
        assert_eq!(frame_pts(&packet(0x100, None)), None);
        let mut not_ts = packet(0x100, Some(5));
        not_ts[0] = 0;
        assert_eq!(frame_pts(&not_ts), None);
    }

    #[test]
    fn the_tail_is_cut_at_the_frame_it_starts_at() {
        let (file, video_at) = recording();
        cut_tail(file.path(), Duration::from_secs(10)).unwrap();
        // The last frame is 749; 10 s is 250 frames back. The audio frame
        // just before it is timed earlier, so it stays.
        let len = std::fs::metadata(file.path()).unwrap().len();
        assert_eq!(len, video_at[499]);
        assert_eq!(len % PACKET, 0);
    }

    #[test]
    fn a_tail_as_long_as_the_file_cuts_nothing() {
        let (file, _) = recording();
        let before = std::fs::metadata(file.path()).unwrap().len();
        cut_tail(file.path(), Duration::from_secs(40)).unwrap();
        cut_tail(file.path(), Duration::ZERO).unwrap();
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), before);

        let other = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(other.path(), vec![7u8; 100_000]).unwrap();
        cut_tail(other.path(), Duration::from_secs(1)).unwrap();
        assert_eq!(std::fs::metadata(other.path()).unwrap().len(), 100_000);
    }
}
