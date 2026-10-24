//! Reading and cutting the MP4s RSnip records: one video track, as GStreamer's mp4mux (Linux) and
//! Media Foundation (Windows) write them. A cut copies the frames it keeps into a new file without
//! re-encoding them, so it's instant and loses nothing, but it starts on a keyframe: a frame the
//! ones after it are drawn from. Recordings have one every second.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

/// A recording's video track: its frames, and what's needed to write them out again.
pub struct Movie {
    pub width: u32,
    pub height: u32,
    /// The track's time units per second.
    timescale: u32,
    /// The movie header's units per second (for the durations in `mvhd` and `tkhd`).
    movie_timescale: u32,
    /// Each frame, in the order it's decoded.
    samples: Vec<Sample>,
    /// The boxes copied as they are: `ftyp`, and the track's headers and sample description.
    ftyp: Vec<u8>,
    mvhd: Vec<u8>,
    tkhd: Vec<u8>,
    mdhd: Vec<u8>,
    hdlr: Vec<u8>,
    /// `minf`'s boxes besides `stbl` (`vmhd`, `dinf`).
    minf_rest: Vec<u8>,
    stsd: Vec<u8>,
    /// `ctts`'s version, if the frames are shown in another order than they're decoded.
    ctts_version: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    offset: u64,
    size: u32,
    /// When it's decoded, in track units from the start.
    dts: u64,
    duration: u32,
    /// How much later it's shown than decoded.
    cts: i32,
    keyframe: bool,
}

impl Movie {
    pub fn open(path: &Path) -> Result<Movie, String> {
        let mut file = File::open(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let (mut ftyp, mut moov) = (None, None);
        let mut at = 0;
        while at + 8 <= len {
            file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
            let (kind, header, size) = box_header(&mut file, len - at)?;
            match &kind {
                b"ftyp" => ftyp = Some(read_box(&mut file, at, size)?),
                b"moov" => moov = Some(read_box(&mut file, at + header, size - header)?),
                _ => {}
            }
            at += size;
        }
        let moov = moov.ok_or("Not an MP4 RSnip can read: it has no index (moov), so it wasn't finished")?;
        Movie::parse(ftyp.unwrap_or_default(), &moov)
    }

    /// The movie from its `ftyp` box and `moov`'s contents.
    fn parse(ftyp: Vec<u8>, moov: &[u8]) -> Result<Movie, String> {
        let bad = |what: &str| format!("Not an MP4 RSnip can read: {what}");
        if find(moov, b"mvex").is_some() {
            return Err(bad("it's fragmented"));
        }
        // Each header long enough for what's read from it and changed in it.
        let mvhd = header(find(moov, b"mvhd"), 16, 28).ok_or_else(|| bad("no movie header"))?;
        let movie_timescale = be32(full(mvhd), if mvhd[8] == 1 { 16 } else { 8 });
        let trak = children(moov).filter(|(k, _)| k == b"trak").map(|(_, b)| b).find(|t| {
            let hdlr = find(content(t), b"mdia").and_then(|m| find(content(m), b"hdlr"));
            hdlr.is_some_and(|h| content(h).get(8..12) == Some(&b"vide"[..]))
        });
        let trak = content(trak.ok_or_else(|| bad("no video"))?);
        let tkhd = header(find(trak, b"tkhd"), 80, 92).ok_or_else(|| bad("no track header"))?;
        let mdia = content(find(trak, b"mdia").ok_or_else(|| bad("no media"))?);
        let mdhd = header(find(mdia, b"mdhd"), 16, 28).ok_or_else(|| bad("no media header"))?;
        let hdlr = find(mdia, b"hdlr").ok_or_else(|| bad("no handler"))?;
        let minf = content(find(mdia, b"minf").ok_or_else(|| bad("no media information"))?);
        let stbl = content(find(minf, b"stbl").ok_or_else(|| bad("no sample table"))?);
        let table = |kind: &[u8; 4]| find(stbl, kind).map(full);
        let stsd = find(stbl, b"stsd").ok_or_else(|| bad("no sample description"))?;

        // The size is in tkhd's last 8 bytes, as 16.16 fixed point.
        let t = full(tkhd);
        let (width, height) = (be32(t, t.len() - 8) >> 16, be32(t, t.len() - 4) >> 16);
        let timescale = be32(full(mdhd), if mdhd[8] == 1 { 16 } else { 8 });

        let sizes = table(b"stsz").ok_or_else(|| bad("no sample sizes"))?;
        let fixed = be32(sizes, 0);
        let count = be32(sizes, 4) as usize;
        let size = |i: usize| if fixed != 0 { fixed } else { be32(sizes, 8 + 4 * i) };
        if fixed == 0 && sizes.len() < 8 + 4 * count {
            return Err(bad("its sample sizes are cut short"));
        }

        // Where each chunk starts, and how many samples each chunk has.
        let chunks: Vec<u64> = match (table(b"stco"), table(b"co64")) {
            (Some(t), _) => entries(t, 4).map(|e| be32(e, 0) as u64).collect(),
            (_, Some(t)) => entries(t, 8).map(|e| be64(e, 0)).collect(),
            _ => return Err(bad("no chunk offsets")),
        };
        let stsc: Vec<(u32, u32)> = entries(table(b"stsc").ok_or_else(|| bad("no chunk map"))?, 12).map(|e| (be32(e, 0), be32(e, 4))).collect();
        let durations: Vec<(u32, u32)> = entries(table(b"stts").ok_or_else(|| bad("no timing"))?, 8).map(|e| (be32(e, 0), be32(e, 4))).collect();
        let ctts = find(stbl, b"ctts");
        let offsets: Vec<(u32, i32)> = ctts.map(|c| entries(full(c), 8).map(|e| (be32(e, 0), be32(e, 4) as i32)).collect()).unwrap_or_default();
        let keyframes: Option<Vec<u32>> = table(b"stss").map(|t| entries(t, 4).map(|e| be32(e, 0)).collect());

        let mut samples = Vec::with_capacity(count);
        let (mut map, mut timing, mut shift) = (stsc.iter().peekable(), runs(&durations), runs(&offsets));
        let mut per_chunk = 0;
        let mut dts = 0u64;
        'chunks: for (c, &chunk_start) in chunks.iter().enumerate() {
            while let Some(&&(first, n)) = map.peek() {
                if first as usize > c + 1 {
                    break;
                }
                per_chunk = n;
                map.next();
            }
            let mut offset = chunk_start;
            for _ in 0..per_chunk {
                let i = samples.len();
                if i == count {
                    break 'chunks;
                }
                let duration = timing.next().unwrap_or(0);
                let n = i as u32 + 1;
                let keyframe = keyframes.as_ref().is_none_or(|k| k.binary_search(&n).is_ok());
                samples.push(Sample { offset, size: size(i), dts, duration, cts: shift.next().unwrap_or(0), keyframe });
                offset += size(i) as u64;
                dts += duration as u64;
            }
        }
        if samples.len() != count || count == 0 {
            return Err(bad("its frames don't add up"));
        }
        let minf_rest = children(minf).filter(|(k, _)| k != b"stbl").flat_map(|(_, b)| b.to_vec()).collect();
        Ok(Movie {
            width,
            height,
            timescale: timescale.max(1),
            movie_timescale: movie_timescale.max(1),
            samples,
            ftyp,
            mvhd: mvhd.to_vec(),
            tkhd: tkhd.to_vec(),
            mdhd: mdhd.to_vec(),
            hdlr: hdlr.to_vec(),
            minf_rest,
            stsd: stsd.to_vec(),
            ctts_version: ctts.map(|c| c[8]),
        })
    }

    pub fn duration(&self) -> Duration {
        let end = self.samples.last().map_or(0, |s| s.dts + s.duration as u64);
        self.time(end)
    }

    #[cfg(test)]
    pub fn frames(&self) -> usize {
        self.samples.len()
    }

    fn time(&self, units: u64) -> Duration {
        Duration::from_secs_f64(units as f64 / self.timescale as f64)
    }

    /// When each keyframe is, in order.
    #[cfg(test)]
    pub fn keyframes(&self) -> Vec<Duration> {
        self.samples.iter().filter(|s| s.keyframe).map(|s| self.time(s.dts)).collect()
    }

    /// The frames to keep for `from`..`to`: from the last keyframe at or before `from`, to the
    /// last frame before `to`.
    fn span(&self, from: Duration, to: Duration) -> std::ops::Range<usize> {
        let at = |t: Duration| (t.as_secs_f64() * self.timescale as f64).round() as u64;
        let (from, to) = (at(from), at(to));
        let first = self.samples.iter().rposition(|s| s.keyframe && s.dts <= from).unwrap_or(0);
        let end = self.samples.iter().position(|s| s.dts >= to).unwrap_or(self.samples.len()).max(first + 1);
        first..end
    }

    /// Copies `from`..`to` of `src` (this movie's file) to `dst`; returns the times it actually
    /// starts and ends at, the start moved back to a keyframe.
    pub fn cut(&self, src: &Path, dst: &Path, from: Duration, to: Duration) -> Result<(Duration, Duration), String> {
        let span = self.span(from, to);
        let kept = &self.samples[span.clone()];
        let start = kept[0].dts;
        let mut input = File::open(src).map_err(|e| format!("Couldn't open {}: {e}", src.display()))?;
        let out = File::create(dst).map_err(|e| format!("Couldn't write {}: {e}", dst.display()))?;
        let mut out = BufWriter::new(out);
        let write = |out: &mut BufWriter<File>, bytes: &[u8]| out.write_all(bytes).map_err(|e| format!("Couldn't write {}: {e}", dst.display()));

        // The index goes first, so players can start before the end has loaded. Its chunk
        // offsets are 64-bit, so its size doesn't depend on them.
        let media: u64 = kept.iter().map(|s| s.size as u64).sum();
        let moov_len = self.moov(kept, 0).len() as u64;
        let mdat_at = self.ftyp.len() as u64 + moov_len;
        let first_byte = mdat_at + 16;
        let moov = self.moov(kept, first_byte);
        debug_assert_eq!(moov.len() as u64, moov_len);
        write(&mut out, &self.ftyp)?;
        write(&mut out, &moov)?;
        // A 64-bit mdat header, for long recordings.
        write(&mut out, &1u32.to_be_bytes())?;
        write(&mut out, b"mdat")?;
        write(&mut out, &(16 + media).to_be_bytes())?;
        let mut buf = Vec::new();
        for s in kept {
            buf.resize(s.size as usize, 0);
            input.seek(SeekFrom::Start(s.offset)).and_then(|_| input.read_exact(&mut buf)).map_err(|e| format!("Couldn't read {}: {e}", src.display()))?;
            write(&mut out, &buf)?;
        }
        out.flush().map_err(|e| format!("Couldn't write {}: {e}", dst.display()))?;
        let end = kept.last().map_or(start, |s| s.dts + s.duration as u64);
        Ok((self.time(start), self.time(end)))
    }

    /// The `moov` box for the `kept` frames, with their data from `first_byte`.
    fn moov(&self, kept: &[Sample], first_byte: u64) -> Vec<u8> {
        let length: u64 = kept.iter().map(|s| s.duration as u64).sum();
        let movie_length = length * self.movie_timescale as u64 / self.timescale as u64;

        let mut stts = Vec::new();
        for (count, duration) in run_lengths(kept.iter().map(|s| s.duration)) {
            stts.extend([count.to_be_bytes(), duration.to_be_bytes()].concat());
        }
        let mut stbl = self.stsd.clone();
        stbl.extend(full_box(b"stts", 0, &[&(stts.len() as u32 / 8).to_be_bytes()[..], &stts].concat()));
        if let Some(version) = self.ctts_version {
            let mut ctts = Vec::new();
            for (count, cts) in run_lengths(kept.iter().map(|s| s.cts as u32)) {
                ctts.extend([count.to_be_bytes(), cts.to_be_bytes()].concat());
            }
            stbl.extend(full_box(b"ctts", version, &[&(ctts.len() as u32 / 8).to_be_bytes()[..], &ctts].concat()));
        }
        if kept.iter().any(|s| !s.keyframe) {
            let keys: Vec<u8> = (1..).zip(kept).filter(|(_, s)| s.keyframe).flat_map(|(n, _)| (n as u32).to_be_bytes()).collect();
            stbl.extend(full_box(b"stss", 0, &[&(keys.len() as u32 / 4).to_be_bytes()[..], &keys].concat()));
        }
        // A chunk for each frame.
        stbl.extend(full_box(b"stsc", 0, &[1u32, 1, 1, 1].iter().flat_map(|n| n.to_be_bytes()).collect::<Vec<u8>>()));
        let sizes: Vec<u8> = kept.iter().flat_map(|s| s.size.to_be_bytes()).collect();
        stbl.extend(full_box(b"stsz", 0, &[&0u32.to_be_bytes()[..], &(kept.len() as u32).to_be_bytes(), &sizes].concat()));
        let mut offsets = (kept.len() as u32).to_be_bytes().to_vec();
        let mut at = first_byte;
        for s in kept {
            offsets.extend(at.to_be_bytes());
            at += s.size as u64;
        }
        stbl.extend(full_box(b"co64", 0, &offsets));

        let minf = [self.minf_rest.clone(), plain_box(b"stbl", &stbl)].concat();
        let mdia = [with_duration(&self.mdhd, length, 16, 24), self.hdlr.clone(), plain_box(b"minf", &minf)].concat();
        // The track starts at the start (any edit list would be for the old one).
        let trak = [with_duration(&self.tkhd, movie_length, 20, 28), plain_box(b"mdia", &mdia)].concat();
        plain_box(b"moov", &[with_duration(&self.mvhd, movie_length, 16, 24), plain_box(b"trak", &trak)].concat())
    }
}

/// `b`, if it's a full box with at least `v0` bytes after its version and flags (version 0), or
/// `v1` (version 1).
fn header(b: Option<&[u8]>, v0: usize, v1: usize) -> Option<&[u8]> {
    b.filter(|b| b.len() >= 12 && b.len() >= 12 + if b[8] == 1 { v1 } else { v0 })
}

/// A header box with its duration changed: at `v0` bytes into its contents for version 0 (32-bit),
/// `v1` for version 1 (64-bit).
fn with_duration(header: &[u8], duration: u64, v0: usize, v1: usize) -> Vec<u8> {
    let mut b = header.to_vec();
    if b[8] == 1 {
        b[8 + v1..16 + v1].copy_from_slice(&duration.to_be_bytes());
    } else {
        b[8 + v0..12 + v0].copy_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
    }
    b
}

/// Equal values in a row, as (how many, value).
fn run_lengths(values: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for v in values {
        match runs.last_mut() {
            Some((n, last)) if *last == v => *n += 1,
            _ => runs.push((1, v)),
        }
    }
    runs
}

/// (how many, value) runs, one value at a time.
fn runs<T: Copy>(table: &[(u32, T)]) -> impl Iterator<Item = T> + '_ {
    table.iter().flat_map(|&(n, v)| std::iter::repeat_n(v, n as usize))
}

fn plain_box(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
    [&(8 + content.len() as u32).to_be_bytes()[..], kind, content].concat()
}

fn full_box(kind: &[u8; 4], version: u8, content: &[u8]) -> Vec<u8> {
    plain_box(kind, &[&[version, 0, 0, 0][..], content].concat())
}

/// A box's type, header length and whole length, at the reader's position, with `left` bytes left.
fn box_header(r: &mut impl Read, left: u64) -> Result<([u8; 4], u64, u64), String> {
    let mut h = [0u8; 8];
    r.read_exact(&mut h).map_err(|e| e.to_string())?;
    let kind = [h[4], h[5], h[6], h[7]];
    let (header, size) = match be32(&h, 0) {
        // To the end of the file.
        0 => (8, left),
        1 => {
            let mut big = [0u8; 8];
            r.read_exact(&mut big).map_err(|e| e.to_string())?;
            (16, u64::from_be_bytes(big))
        }
        n => (8, n as u64),
    };
    if size < header || size > left {
        return Err("Not an MP4 RSnip can read: a box runs past the end".into());
    }
    Ok((kind, header, size))
}

fn read_box(file: &mut File, at: u64, len: u64) -> Result<Vec<u8>, String> {
    if len > 256 << 20 {
        return Err("Not an MP4 RSnip can read: its index is too big".into());
    }
    let mut buf = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(at)).and_then(|_| file.read_exact(&mut buf)).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// The boxes in `data`, as (type, the whole box).
fn children(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let size = be32(data.get(at..at + 4)?, 0) as usize;
        let kind: [u8; 4] = data.get(at + 4..at + 8)?.try_into().ok()?;
        let (size, header) = match size {
            0 => (data.len() - at, 8),
            1 => (be64(data.get(at + 8..at + 16)?, 0) as usize, 16),
            n => (n, 8),
        };
        if size < header || at + size > data.len() {
            return None;
        }
        let whole = &data[at..at + size];
        at += size;
        Some((kind, whole))
    })
}

fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(data).find(|(k, _)| k == kind).map(|(_, b)| b)
}

/// A box's contents, past its 8-byte header.
fn content(b: &[u8]) -> &[u8] {
    &b[8..]
}

/// A full box's contents past its version and flags too.
fn full(b: &[u8]) -> &[u8] {
    b.get(12..).unwrap_or_default()
}

/// A table's entries of `size` bytes, after its 4-byte count.
fn entries(table: &[u8], size: usize) -> impl Iterator<Item = &[u8]> {
    let count = table.get(..4).map_or(0, |c| be32(c, 0) as usize);
    table.get(4..).unwrap_or_default().chunks_exact(size).take(count)
}

fn be32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_be_bytes(s.try_into().unwrap()))
}

fn be64(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).map_or(0, |s| u64::from_be_bytes(s.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A movie of `frames` frames at 30 per second, a keyframe every `every`, each frame's data
    /// its number repeated.
    fn movie(frames: u32, every: u32) -> (Movie, Vec<u8>) {
        let timescale: u32 = 90_000;
        let samples: Vec<Sample> = (0..frames)
            .map(|n| Sample { offset: 0, size: 10 + n % 7, dts: n as u64 * 3000, duration: 3000, cts: 0, keyframe: n % every == 0 })
            .collect();
        let stsd = full_box(b"stsd", 0, &[&1u32.to_be_bytes()[..], &plain_box(b"avc1", &[0u8; 78])].concat());
        let tkhd = full_box(b"tkhd", 0, &[&[0u8; 72][..], &(640u32 << 16).to_be_bytes(), &(480u32 << 16).to_be_bytes()].concat());
        let mut mvhd_content = vec![0u8; 96];
        mvhd_content[8..12].copy_from_slice(&1000u32.to_be_bytes());
        let mut mdhd_content = vec![0u8; 20];
        mdhd_content[8..12].copy_from_slice(&timescale.to_be_bytes());
        let movie = Movie {
            width: 640,
            height: 480,
            timescale,
            movie_timescale: 1000,
            samples,
            ftyp: plain_box(b"ftyp", b"isom\0\0\0\0isom"),
            mvhd: full_box(b"mvhd", 0, &mvhd_content),
            tkhd,
            mdhd: full_box(b"mdhd", 0, &mdhd_content),
            hdlr: full_box(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0u8; 13]].concat()),
            minf_rest: full_box(b"vmhd", 0, &[0u8; 8]),
            stsd,
            ctts_version: None,
        };
        let data: Vec<u8> = (0..frames).flat_map(|n| std::iter::repeat_n(n as u8, 10 + (n % 7) as usize)).collect();
        (movie, data)
    }

    /// Writes `movie` with `data` as an MP4 at `path`, the way a cut does, with nothing cut.
    fn write(movie: &mut Movie, data: &[u8], path: &Path) {
        let mut at = 0;
        for s in &mut movie.samples {
            s.offset = at;
            at += s.size as u64;
        }
        let raw = path.with_extension("raw");
        std::fs::write(&raw, data).unwrap();
        movie.cut(&raw, path, Duration::ZERO, Duration::MAX).unwrap();
    }

    #[test]
    fn cuts_from_a_keyframe_and_keeps_the_frames_as_they_were() {
        let dir = std::env::temp_dir().join(format!("rsnip-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut movie, data) = movie(90, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);

        let read = Movie::open(&whole).unwrap();
        assert_eq!((read.width, read.height, read.frames()), (640, 480, 90));
        assert!((read.duration().as_secs_f64() - 3.0).abs() < 1e-6);
        assert_eq!(read.keyframes(), [0.0, 1.0, 2.0].map(Duration::from_secs_f64));

        // 1.5 s to 2.5 s starts at the keyframe at 1 s.
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut(&whole, &cut, Duration::from_secs_f64(1.5), Duration::from_secs_f64(2.5)).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs_f64(2.5)));
        let part = Movie::open(&cut).unwrap();
        assert_eq!(part.frames(), 45);
        assert!((part.duration().as_secs_f64() - 1.5).abs() < 1e-6);
        assert!(part.samples[0].keyframe && part.samples[30].keyframe && !part.samples[1].keyframe);
        // The frames are the 30th to the 74th, byte for byte.
        let bytes = std::fs::read(&cut).unwrap();
        for (n, s) in (30u8..).zip(&part.samples) {
            let frame = &bytes[s.offset as usize..(s.offset + s.size as u64) as usize];
            assert!(frame.iter().all(|&b| b == n) && s.size == 10 + n as u32 % 7, "frame {n}");
        }
        // The headers say how long it is.
        let m = Movie::open(&cut).unwrap();
        assert_eq!(be32(full(&m.mvhd), 12), 1500, "mvhd, in ms");
        assert_eq!(be32(full(&m.mdhd), 12), 135_000, "mdhd, in track units");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_keeps_at_least_a_frame_and_stays_inside_the_movie() {
        let (movie, _) = movie(60, 30);
        assert_eq!(movie.span(Duration::from_secs(5), Duration::from_secs(9)), 30..60, "past the end: the last second");
        assert_eq!(movie.span(Duration::ZERO, Duration::ZERO), 0..1);
        assert_eq!(movie.span(Duration::from_secs_f64(0.99), Duration::from_secs_f64(1.0)), 0..30);
    }

    #[test]
    fn refuses_what_it_cant_read() {
        assert!(Movie::parse(Vec::new(), b"").is_err());
        assert!(Movie::parse(Vec::new(), &plain_box(b"mvex", b"")).is_err());
        let path = std::env::temp_dir().join(format!("rsnip-not-mp4-{}", std::process::id()));
        std::fs::write(&path, b"\0\0\0\x10junkjunkjunk").unwrap();
        assert!(Movie::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    proptest::proptest! {
        /// Damaged indexes are refused or read, never a panic.
        #[test]
        fn damaged_indexes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..400), cut in 0usize..400) {
            let (mut movie, data) = movie(40, 10);
            let path = std::env::temp_dir().join(format!("rsnip-damaged-{}-{}", std::process::id(), cut));
            write(&mut movie, &data, &path);
            let mut file = std::fs::read(&path).unwrap();
            let at = cut.min(file.len());
            let end = (at + bytes.len()).min(file.len());
            file[at..end].copy_from_slice(&bytes[..end - at]);
            std::fs::write(&path, &file).unwrap();
            let _ = Movie::open(&path).map(|m| m.span(Duration::from_secs(1), Duration::from_secs(2)));
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("raw"));
        }
    }
}
