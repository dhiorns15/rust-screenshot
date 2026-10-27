//! Reading and cutting the MP4s RSnip records: a video track, and a sound track if there is one,
//! as GStreamer's mp4mux (Linux) and Media Foundation (Windows) write them. A cut copies the frames
//! it keeps into a new file without re-encoding them, so it's instant and loses nothing, but it
//! starts on a keyframe: a frame the ones after it are drawn from. Recordings have one every second.
//! Times are when things are shown, by the tracks' edit lists: mp4mux's sound often starts a little
//! after the picture, and a cut keeps it that much later.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

/// A recording: its video track, any sound tracks, and what's needed to write them out again.
pub struct Movie {
    pub width: u32,
    pub height: u32,
    /// The movie header's units per second (for the durations in `mvhd` and `tkhd`).
    movie_timescale: u32,
    /// The boxes copied as they are: `ftyp` and `mvhd`.
    ftyp: Vec<u8>,
    mvhd: Vec<u8>,
    /// The video: it decides what a cut keeps.
    video: Track,
    /// The sound tracks that could be read. Tracks of any other kind (text, hints) are left out.
    sound: Vec<Track>,
}

/// A track's samples (a video track's frames, a sound track's audio frames), and what's needed to
/// write them out again.
struct Track {
    /// The track's time units per second.
    timescale: u32,
    /// How much later than its media time each sample is shown, in track units, from the edit
    /// list: an empty edit delays a track that started later than the others (as mp4mux writes
    /// for sound that starts after the picture), and a media time skips the start of its media
    /// (as B-frames need).
    shift: i64,
    /// When the edit list stops showing it, in track units, if it says: a cut's video can have
    /// frames after the end that B-frames are drawn from.
    until: Option<i64>,
    /// Each sample, in the order it's decoded.
    samples: Vec<Sample>,
    /// The boxes copied as they are: the track's headers and sample description.
    tkhd: Vec<u8>,
    mdhd: Vec<u8>,
    hdlr: Vec<u8>,
    /// `minf`'s boxes besides `stbl` (`vmhd` or `smhd`, and `dinf`).
    minf_rest: Vec<u8>,
    stsd: Vec<u8>,
    /// `ctts`'s version, if the samples are shown in another order than they're decoded.
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

fn bad(what: &str) -> String {
    format!("Not an MP4 RSnip can read: {what}")
}

/// What kind of media a `trak` box has ("vide", "soun"), from its handler.
fn handler(trak: &[u8]) -> Option<&[u8]> {
    let hdlr = find(content(trak), b"mdia").and_then(|m| find(content(m), b"hdlr"))?;
    content(hdlr).get(8..12)
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
        let mut movie = Movie::parse(ftyp.unwrap_or_default(), &moov)?;
        // A sound track with samples outside the file is left out, rather than failing a cut half
        // way through.
        movie.sound.retain(|t| t.samples.iter().all(|s| s.offset.checked_add(s.size as u64).is_some_and(|end| end <= len)));
        Ok(movie)
    }

    /// The movie from its `ftyp` box and `moov`'s contents.
    fn parse(ftyp: Vec<u8>, moov: &[u8]) -> Result<Movie, String> {
        if find(moov, b"mvex").is_some() {
            return Err(bad("it's fragmented"));
        }
        // Each header long enough for what's read from it and changed in it.
        let mvhd = header(find(moov, b"mvhd"), 16, 28).ok_or_else(|| bad("no movie header"))?;
        let movie_timescale = be32(full(mvhd), if mvhd[8] == 1 { 16 } else { 8 });
        let traks: Vec<&[u8]> = children(moov).filter(|(k, _)| k == b"trak").map(|(_, b)| b).collect();
        let video = traks.iter().find(|t| handler(t) == Some(&b"vide"[..]));
        let movie_timescale = movie_timescale.max(1);
        let video = Track::parse(video.ok_or_else(|| bad("no video"))?, movie_timescale)?;
        // The size is in tkhd's last 8 bytes, as 16.16 fixed point.
        let t = full(&video.tkhd);
        let (width, height) = (be32(t, t.len() - 8) >> 16, be32(t, t.len() - 4) >> 16);

        // A sound track that can't be read is left out: the video is still worth keeping. Each
        // track needs a number of its own; if two have the same, the first keeps it.
        let mut ids = vec![video.id()];
        let mut sound = Vec::new();
        for trak in traks.iter().filter(|t| handler(t) == Some(&b"soun"[..])) {
            if let Ok(track) = Track::parse(trak, movie_timescale) {
                if track.id() != 0 && !ids.contains(&track.id()) {
                    ids.push(track.id());
                    sound.push(track);
                }
            }
        }
        Ok(Movie { width, height, movie_timescale, ftyp, mvhd: mvhd.to_vec(), video, sound })
    }

    /// How long the picture lasts: until its last frame is shown and over.
    pub fn duration(&self) -> Duration {
        let video = &self.video;
        let end = video.samples.iter().map(|s| video.shown(s) + s.duration as i64).max().unwrap_or(0);
        self.time(video.until.map_or(end, |until| end.min(until)))
    }

    /// Whether it has a sound track.
    pub fn has_sound(&self) -> bool {
        !self.sound.is_empty()
    }

    #[cfg(test)]
    pub fn frames(&self) -> usize {
        self.video.samples.len()
    }

    /// `units` of the video's time as a duration (none before the start).
    fn time(&self, units: i64) -> Duration {
        Duration::from_secs_f64(units.max(0) as f64 / self.video.timescale as f64)
    }

    /// `t` in units of the video's time.
    fn units(&self, t: Duration) -> i64 {
        (t.as_secs_f64() * self.video.timescale as f64).round() as i64
    }

    /// When each keyframe is shown, in order.
    #[cfg(test)]
    pub fn keyframes(&self) -> Vec<Duration> {
        self.video.samples.iter().filter(|s| s.keyframe).map(|s| self.time(self.video.shown(s))).collect()
    }

    /// When each sample of each track is shown, in seconds: the video's, then each sound track's.
    #[cfg(test)]
    pub fn shown(&self) -> Vec<Vec<f64>> {
        std::iter::once(&self.video).chain(&self.sound).map(|t| t.samples.iter().map(|s| t.shown(s) as f64 / t.timescale as f64).collect()).collect()
    }

    /// The frames to keep for `from`..`to`: from the last keyframe shown at or before `from`, to
    /// the last frame shown before `to`, in the order they're decoded.
    fn span(&self, from: Duration, to: Duration) -> Range<usize> {
        let (video, samples) = (&self.video, &self.video.samples);
        let (from, to) = (self.units(from), self.units(to));
        let first = samples.iter().rposition(|s| s.keyframe && video.shown(s) <= from).unwrap_or(0);
        // With B-frames, that keeps the frame after them they're drawn from, though it's shown
        // from `to` on.
        let end = samples[first..].iter().rposition(|s| video.shown(s) < to).map_or(first + 1, |n| first + n + 1);
        first..end
    }

    /// Copies `from`..`to` of `src` (this movie's file) to `dst`; returns the times it actually
    /// starts and ends at, the start moved back to a keyframe. The sound tracks keep the audio
    /// frames that start inside that, each shown as long after the start as it was before.
    pub fn cut(&self, src: &Path, dst: &Path, from: Duration, to: Duration) -> Result<(Duration, Duration), String> {
        self.cut_tracks(src, dst, from, to, true)
    }

    /// `cut`, with the sound tracks only if `sound`: decoding the picture needn't copy the sound.
    /// The video is cut the same either way.
    pub fn cut_tracks(&self, src: &Path, dst: &Path, from: Duration, to: Duration, sound: bool) -> Result<(Duration, Duration), String> {
        let span = self.span(from, to);
        let video = &self.video.samples[span];
        let start = self.video.shown(&video[0]);
        // It ends when the last frame shown before `to` does: a frame kept only for B-frames to be
        // drawn from isn't shown.
        let over = |s: &Sample| self.video.shown(s) + s.duration as i64;
        let before = video.iter().filter(|s| self.video.shown(s) < self.units(to)).map(over).max();
        let end = before.unwrap_or_else(|| video.iter().map(over).max().unwrap_or(start));
        let end = self.video.until.map_or(end, |until| end.min(until).max(start));

        let mut parts = vec![Part::new(&self.video, video, true, (start, Some(end)), self.video.timescale)];
        for track in self.sound.iter().filter(|_| sound) {
            let kept = track.within(start, end, self.video.timescale);
            if !kept.is_empty() {
                parts.push(Part::new(track, kept, false, (start, None), self.video.timescale));
            }
        }
        // Each second's data of each track goes together, as players like to read it, the video's
        // first. Only the order of the data and the chunk offsets depend on this.
        let mut order: Vec<(u64, usize, usize)> = parts.iter().enumerate().flat_map(|(p, part)| part.chunks.iter().enumerate().map(move |(c, chunk)| (chunk.0, p, c))).collect();
        order.sort_unstable();

        let mut input = File::open(src).map_err(|e| format!("Couldn't open {}: {e}", src.display()))?;
        let out = File::create(dst).map_err(|e| format!("Couldn't write {}: {e}", dst.display()))?;
        let result = (|| {
            let mut out = BufWriter::new(out);
            let write = |out: &mut BufWriter<File>, bytes: &[u8]| out.write_all(bytes).map_err(|e| format!("Couldn't write {}: {e}", dst.display()));

            // The index goes first, so players can start before the end has loaded. Its chunk
            // offsets are 64-bit, so its size doesn't depend on them.
            let media: u64 = parts.iter().flat_map(|p| p.kept).map(|s| s.size as u64).sum();
            let moov_len = self.moov(&parts).len() as u64;
            let mut at = self.ftyp.len() as u64 + moov_len + 16;
            for &(_, p, c) in &order {
                let size: u64 = parts[p].samples(c).iter().map(|s| s.size as u64).sum();
                parts[p].offsets[c] = at;
                at += size;
            }
            let moov = self.moov(&parts);
            debug_assert_eq!(moov.len() as u64, moov_len);
            write(&mut out, &self.ftyp)?;
            write(&mut out, &moov)?;
            // A 64-bit mdat header, for long recordings.
            write(&mut out, &1u32.to_be_bytes())?;
            write(&mut out, b"mdat")?;
            write(&mut out, &(16 + media).to_be_bytes())?;
            let mut buf = Vec::new();
            for &(_, p, c) in &order {
                for s in parts[p].samples(c) {
                    buf.resize(s.size as usize, 0);
                    input.seek(SeekFrom::Start(s.offset)).and_then(|_| input.read_exact(&mut buf)).map_err(|e| format!("Couldn't read {}: {e}", src.display()))?;
                    write(&mut out, &buf)?;
                }
            }
            out.flush().map_err(|e| format!("Couldn't write {}: {e}", dst.display()))
        })();
        if let Err(e) = result {
            // Not a half-written file.
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
        Ok((self.time(start), self.time(end)))
    }

    /// The `moov` box for the `parts` kept, with their data where each part's `offsets` say.
    fn moov(&self, parts: &[Part]) -> Vec<u8> {
        let length = |p: &Part| p.edits(self.movie_timescale).iter().map(|e| e.0).sum::<u64>();
        let movie_length = parts.iter().map(length).max().unwrap_or(0);
        let mut mvhd = with_duration(&self.mvhd, movie_length, 16, 24);
        // The next track number goes past the ones used. It's the last thing in the header, so
        // there if the header is long enough.
        let next = parts.iter().map(|p| p.track.id()).max().map_or(1, |id| id.saturating_add(1));
        let at = if mvhd[8] == 1 { 116 } else { 104 };
        if mvhd.len() >= at + 4 && be32(&mvhd, at) < next {
            mvhd[at..at + 4].copy_from_slice(&next.to_be_bytes());
        }
        let traks: Vec<u8> = parts.iter().flat_map(|p| p.trak(length(p), self.movie_timescale)).collect();
        plain_box(b"moov", &[mvhd, traks].concat())
    }
}

impl Track {
    /// A `trak` box's samples and the boxes to write them out again, in a movie of
    /// `movie_timescale` units per second.
    fn parse(trak: &[u8], movie_timescale: u32) -> Result<Track, String> {
        let trak = content(trak);
        let tkhd = header(find(trak, b"tkhd"), 80, 92).ok_or_else(|| bad("no track header"))?;
        let mdia = content(find(trak, b"mdia").ok_or_else(|| bad("no media"))?);
        let mdhd = header(find(mdia, b"mdhd"), 16, 28).ok_or_else(|| bad("no media header"))?;
        let hdlr = find(mdia, b"hdlr").ok_or_else(|| bad("no handler"))?;
        let minf = content(find(mdia, b"minf").ok_or_else(|| bad("no media information"))?);
        let stbl = content(find(minf, b"stbl").ok_or_else(|| bad("no sample table"))?);
        let table = |kind: &[u8; 4]| find(stbl, kind).map(full);
        let stsd = find(stbl, b"stsd").ok_or_else(|| bad("no sample description"))?;
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
        // Sound has none: every sample is a sync sample.
        let keyframes: Option<Vec<u32>> = table(b"stss").map(|t| entries(t, 4).map(|e| be32(e, 0)).collect());

        // (The count is from the file, so it doesn't decide how much to reserve.)
        let mut samples = Vec::with_capacity(count.min(1 << 20));
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
                offset = offset.saturating_add(size(i) as u64);
                dts += duration as u64;
            }
        }
        if samples.len() != count || count == 0 {
            return Err(bad("its frames don't add up"));
        }
        let minf_rest = children(minf).filter(|(k, _)| k != b"stbl").flat_map(|(_, b)| b.to_vec()).collect();
        let elst = find(trak, b"edts").and_then(|e| find(content(e), b"elst"));
        let (shift, until) = elst.map_or((0, None), |e| edit_list(e, movie_timescale, timescale.max(1)));
        Ok(Track {
            timescale: timescale.max(1),
            shift,
            until,
            samples,
            tkhd: tkhd.to_vec(),
            mdhd: mdhd.to_vec(),
            hdlr: hdlr.to_vec(),
            minf_rest,
            stsd: stsd.to_vec(),
            ctts_version: ctts.map(|c| c[8]),
        })
    }

    /// The track's number, from `tkhd`.
    fn id(&self) -> u32 {
        be32(full(&self.tkhd), if self.tkhd[8] == 1 { 16 } else { 8 })
    }

    /// When `s` is shown, in track units from the start of the movie.
    fn shown(&self, s: &Sample) -> i64 {
        s.dts as i64 + s.cts as i64 + self.shift
    }

    /// The samples shown from `from` up to `to`, which are in units of `1 / scale` seconds.
    fn within(&self, from: i64, to: i64, scale: u32) -> &[Sample] {
        let before = |t: i64| self.samples.partition_point(|s| (self.shown(s) as i128 * scale as i128) < t as i128 * self.timescale as i128);
        &self.samples[before(from)..before(to)]
    }
}

/// What a track's edit list (`elst`, in a movie of `movie_timescale`) says, in its units
/// (`timescale` a second): how much later than its media time its samples are shown (the empty
/// edits before the first real one, less that one's media time), and when it stops being shown,
/// if it says. Edits after the first real one (a track shown in pieces, or at another speed)
/// aren't kept to, as RSnip's recordings have none.
fn edit_list(elst: &[u8], movie_timescale: u32, timescale: u32) -> (i64, Option<i64>) {
    let v1 = elst.get(8) == Some(&1);
    // Far past any recording's length, but not so far that adding it to a sample's time overflows.
    let units = |length: u64| scale(length, movie_timescale, timescale).min(1 << 40) as i64;
    let (mut delay, mut until) = (0i64, None);
    let mut media_time = 0;
    for e in entries(full(elst), if v1 { 20 } else { 12 }) {
        let (length, time) = if v1 { (be64(e, 0), be64(e, 8) as i64) } else { (be32(e, 0) as u64, be32(e, 4) as i32 as i64) };
        if time != -1 {
            media_time = time.clamp(0, 1 << 40);
            // A length of 0 is "all of it".
            until = (length > 0).then(|| delay + units(length));
            break;
        }
        delay = (delay + units(length)).min(1 << 40);
    }
    (delay - media_time, until)
}

/// A track's samples that a cut keeps, and how they're laid out in the new file.
struct Part<'a> {
    track: &'a Track,
    kept: &'a [Sample],
    /// The chunks the samples are stored in, as (the second they start in, which samples).
    chunks: Vec<(u64, Range<usize>)>,
    /// Where each chunk starts in the new file.
    offsets: Vec<u64>,
    /// How much later than its new media time each sample is shown in the new file, in track
    /// units: what its edit list says.
    shift: i64,
    /// How long after the start the cut ends, in track units, where it can end before the last
    /// sample does: a video's, where the frames B-frames are drawn from are kept but not shown.
    until: Option<i64>,
}

impl<'a> Part<'a> {
    /// `kept`, in a chunk for each sample if `each` (a video's frames are big), or else a chunk
    /// for each second, for a cut that starts at `start`, and is cut short at `end` if there's one
    /// (in units of `1 / scale` seconds).
    fn new(track: &'a Track, kept: &'a [Sample], each: bool, (start, end): (i64, Option<i64>), scale: u32) -> Part<'a> {
        let mut chunks: Vec<(u64, Range<usize>)> = Vec::new();
        for (i, s) in kept.iter().enumerate() {
            let second = (s.dts - kept[0].dts) / track.timescale as u64;
            match chunks.last_mut() {
                Some((last, range)) if *last == second && !each => range.end = i + 1,
                _ => chunks.push((second, i..i + 1)),
            }
        }
        // Each sample is shown as long after the start as it was before: its media time starts
        // again from 0, so it's shown as much later as it was, less the start.
        let units = |t: i64| (t as i128 * track.timescale as i128).div_euclid(scale.max(1) as i128) as i64;
        let shift = kept[0].dts as i64 + track.shift - units(start);
        Part { track, kept, offsets: vec![0; chunks.len()], chunks, shift, until: end.map(|end| units(end - start)) }
    }

    /// How long its samples last, in track units: until the last is over, from the media time
    /// the first is shown at.
    fn lasts(&self) -> i64 {
        let end = self.kept.iter().map(|s| (s.dts - self.kept[0].dts) as i64 + s.cts as i64 + s.duration as i64).max().unwrap_or(0);
        (end + self.shift.min(0)).max(0)
    }

    /// How long they're shown, in track units: as long as they last, or until the cut ends.
    fn shown(&self) -> i64 {
        self.until.map_or(self.lasts(), |until| self.lasts().min(until - self.shift.max(0)).max(0))
    }

    /// Whether it needs an edit list: unless it's shown from its start at the start, to its end.
    fn edited(&self) -> bool {
        self.shift != 0 || self.shown() < self.lasts()
    }

    /// The edit list's entries, as (how long, in units of `1 / movie_timescale` seconds; its media
    /// time, -1 for an empty edit): an empty edit if the track starts later than the cut, then the
    /// samples from the media time they're first shown at.
    fn edits(&self, movie_timescale: u32) -> Vec<(u64, i64)> {
        // Rounded up, so that read back it still shows all of the last sample.
        let shown = (self.shown() as u128 * movie_timescale as u128).div_ceil(self.track.timescale as u128).min(u64::MAX as u128) as u64;
        match self.shift {
            0 => vec![(shown, 0)],
            ..0 => vec![(shown, -self.shift)],
            _ => match scale(self.shift as u64, self.track.timescale, movie_timescale) {
                // Less than the movie's units can say.
                0 => vec![(shown, 0)],
                delay => vec![(delay, -1), (shown, 0)],
            },
        }
    }

    /// The samples in chunk `c`.
    fn samples(&self, c: usize) -> &'a [Sample] {
        &self.kept[self.chunks[c].1.clone()]
    }

    /// How long the samples last, in track units.
    fn length(&self) -> u64 {
        self.kept.iter().map(|s| s.duration as u64).sum()
    }

    /// The `trak` box, `movie_length` long (in the movie's units, `movie_timescale` a second).
    fn trak(&self, movie_length: u64, movie_timescale: u32) -> Vec<u8> {
        let (track, kept) = (self.track, self.kept);
        let mut stts = Vec::new();
        for (count, duration) in run_lengths(kept.iter().map(|s| s.duration)) {
            stts.extend([count.to_be_bytes(), duration.to_be_bytes()].concat());
        }
        let mut stbl = track.stsd.clone();
        stbl.extend(full_box(b"stts", 0, &[&(stts.len() as u32 / 8).to_be_bytes()[..], &stts].concat()));
        if let Some(version) = track.ctts_version {
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
        // How many samples the chunks have, a row for each change.
        let mut stsc = Vec::new();
        for (n, (_, range)) in self.chunks.iter().enumerate() {
            if n == 0 || self.chunks[n - 1].1.len() != range.len() {
                stsc.extend([n as u32 + 1, range.len() as u32, 1].iter().flat_map(|n| n.to_be_bytes()));
            }
        }
        stbl.extend(full_box(b"stsc", 0, &[&(stsc.len() as u32 / 12).to_be_bytes()[..], &stsc].concat()));
        let sizes: Vec<u8> = kept.iter().flat_map(|s| s.size.to_be_bytes()).collect();
        stbl.extend(full_box(b"stsz", 0, &[&0u32.to_be_bytes()[..], &(kept.len() as u32).to_be_bytes(), &sizes].concat()));
        let offsets: Vec<u8> = self.offsets.iter().flat_map(|o| o.to_be_bytes()).collect();
        stbl.extend(full_box(b"co64", 0, &[&(self.offsets.len() as u32).to_be_bytes()[..], &offsets].concat()));

        let minf = [track.minf_rest.clone(), plain_box(b"stbl", &stbl)].concat();
        let mdia = [with_duration(&track.mdhd, self.length(), 16, 24), track.hdlr.clone(), plain_box(b"minf", &minf)].concat();
        // An edit list, unless the track is shown from its start at the start, so that sound that
        // started later than the picture still does.
        let edts = if self.edited() {
            let edits = self.edits(movie_timescale);
            let entries: Vec<u8> = edits.iter().flat_map(|&(length, media_time)| [(length.min(u32::MAX as u64) as u32).to_be_bytes(), (media_time.clamp(-1, i32::MAX as i64) as i32).to_be_bytes(), 0x0001_0000u32.to_be_bytes()].concat()).collect();
            plain_box(b"edts", &full_box(b"elst", 0, &[&(edits.len() as u32).to_be_bytes()[..], &entries].concat()))
        } else {
            Vec::new()
        };
        plain_box(b"trak", &[with_duration(&track.tkhd, movie_length, 20, 28), edts, plain_box(b"mdia", &mdia)].concat())
    }
}

/// `units` of `1 / from` seconds, in units of `1 / to`.
fn scale(units: u64, from: u32, to: u32) -> u64 {
    (units as u128 * to as u128 / from.max(1) as u128).min(u64::MAX as u128) as u64
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
pub(crate) mod tests {
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
            movie_timescale: 1000,
            ftyp: plain_box(b"ftyp", b"isom\0\0\0\0isom"),
            mvhd: full_box(b"mvhd", 0, &mvhd_content),
            video: Track {
                shift: 0,
                until: None,
                timescale,
                samples,
                tkhd: with_id(tkhd, 1),
                mdhd: full_box(b"mdhd", 0, &mdhd_content),
                hdlr: full_box(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0u8; 13]].concat()),
                minf_rest: full_box(b"vmhd", 0, &[0u8; 8]),
                stsd,
                ctts_version: None,
            },
            sound: Vec::new(),
        };
        let data: Vec<u8> = (0..frames).flat_map(|n| std::iter::repeat_n(n as u8, 10 + (n % 7) as usize)).collect();
        (movie, data)
    }

    /// `tkhd` with its track number set (version 0).
    fn with_id(mut tkhd: Vec<u8>, id: u32) -> Vec<u8> {
        tkhd[20..24].copy_from_slice(&id.to_be_bytes());
        tkhd
    }

    /// Audio sample `n`'s data: its number, counting down so that it differs from a frame's.
    fn audio_byte(n: u32) -> u8 {
        255 - (n % 200) as u8
    }

    /// A movie with a sound track too: `frames` AAC-sized frames of 1024 samples at 48 kHz
    /// (so 21.3 ms each), each its number's bytes. The data is the video's, then the sound's.
    fn movie_with_sound(frames: u32, audio_frames: u32, every: u32) -> (Movie, Vec<u8>) {
        let (mut movie, mut data) = movie(frames, every);
        let samples: Vec<Sample> =
            (0..audio_frames).map(|n| Sample { offset: 0, size: 20 + n % 5, dts: n as u64 * 1024, duration: 1024, cts: 0, keyframe: true }).collect();
        let mut mdhd_content = vec![0u8; 20];
        mdhd_content[8..12].copy_from_slice(&48_000u32.to_be_bytes());
        let tkhd = full_box(b"tkhd", 0, &[&[0u8; 72][..], &[0u8; 8]].concat());
        movie.sound.push(Track {
            shift: 0,
            until: None,
            timescale: 48_000,
            samples,
            tkhd: with_id(tkhd, 2),
            mdhd: full_box(b"mdhd", 0, &mdhd_content),
            hdlr: full_box(b"hdlr", 0, &[&[0u8; 4][..], b"soun", &[0u8; 13]].concat()),
            minf_rest: full_box(b"smhd", 0, &[0u8; 4]),
            stsd: full_box(b"stsd", 0, &[&1u32.to_be_bytes()[..], &plain_box(b"mp4a", &[0u8; 28])].concat()),
            ctts_version: None,
        });
        data.extend((0..audio_frames).flat_map(|n| std::iter::repeat_n(audio_byte(n), 20 + (n % 5) as usize)));
        (movie, data)
    }

    /// A 3 s MP4 at `path` made of nothing but the right headers (it won't play), with a sound
    /// track too if `sound`.
    pub(crate) fn recording(path: &Path, sound: bool) {
        let (mut movie, data) = if sound { movie_with_sound(90, 141, 30) } else { movie(90, 30) };
        write(&mut movie, &data, path);
    }

    /// Writes `movie` with `data` as an MP4 at `path`, the way a cut does, with nothing cut.
    fn write(movie: &mut Movie, data: &[u8], path: &Path) {
        let mut at = 0;
        for s in movie.video.samples.iter_mut().chain(movie.sound.iter_mut().flat_map(|t| t.samples.iter_mut())) {
            s.offset = at;
            at += s.size as u64;
        }
        let raw = path.with_extension("raw");
        std::fs::write(&raw, data).unwrap();
        movie.cut(&raw, path, Duration::ZERO, Duration::MAX).unwrap();
        let _ = std::fs::remove_file(&raw);
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-mp4-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cuts_from_a_keyframe_and_keeps_the_frames_as_they_were() {
        let dir = temp_dir("cut");
        let (mut movie, data) = movie(90, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);

        let read = Movie::open(&whole).unwrap();
        assert_eq!((read.width, read.height, read.frames()), (640, 480, 90));
        assert!(!read.has_sound());
        assert!((read.duration().as_secs_f64() - 3.0).abs() < 1e-6);
        assert_eq!(read.keyframes(), [0.0, 1.0, 2.0].map(Duration::from_secs_f64));

        // 1.5 s to 2.5 s starts at the keyframe at 1 s.
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut(&whole, &cut, Duration::from_secs_f64(1.5), Duration::from_secs_f64(2.5)).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs_f64(2.5)));
        let part = Movie::open(&cut).unwrap();
        assert_eq!(part.frames(), 45);
        assert!((part.duration().as_secs_f64() - 1.5).abs() < 1e-6);
        let frames = &part.video.samples;
        assert!(frames[0].keyframe && frames[30].keyframe && !frames[1].keyframe);
        // The frames are the 30th to the 74th, byte for byte.
        let bytes = std::fs::read(&cut).unwrap();
        for (n, s) in (30u8..).zip(frames) {
            let frame = &bytes[s.offset as usize..(s.offset + s.size as u64) as usize];
            assert!(frame.iter().all(|&b| b == n) && s.size == 10 + n as u32 % 7, "frame {n}");
        }
        // The headers say how long it is.
        let m = Movie::open(&cut).unwrap();
        assert_eq!(be32(full(&m.mvhd), 12), 1500, "mvhd, in ms");
        assert_eq!(be32(full(&m.video.mdhd), 12), 135_000, "mdhd, in track units");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_keeps_at_least_a_frame_and_stays_inside_the_movie() {
        let (movie, _) = movie(60, 30);
        assert_eq!(movie.span(Duration::from_secs(5), Duration::from_secs(9)), 30..60, "past the end: the last second");
        assert_eq!(movie.span(Duration::ZERO, Duration::ZERO), 0..1);
        assert_eq!(movie.span(Duration::from_secs_f64(0.99), Duration::from_secs_f64(1.0)), 0..30);
    }

    /// The bytes of `track`'s samples in `file`.
    fn bytes_of(file: &[u8], track: &Track) -> Vec<Vec<u8>> {
        track.samples.iter().map(|s| file[s.offset as usize..(s.offset + s.size as u64) as usize].to_vec()).collect()
    }

    #[test]
    fn a_cut_keeps_the_sound_that_goes_with_the_picture() {
        let dir = temp_dir("sound");
        // 3 s of video with a keyframe a second, and 141 audio frames: 3.008 s.
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);

        let read = Movie::open(&whole).unwrap();
        assert!(read.has_sound());
        assert_eq!((read.video.samples.len(), read.sound.len()), (90, 1), "both tracks are read back");
        assert_eq!(read.sound[0].timescale, 48_000);
        assert_eq!((read.video.id(), read.sound[0].id()), (1, 2));
        assert_eq!(read.sound[0].samples.len(), 141);
        assert_eq!(be32(&read.mvhd, 104), 3, "the next track number is past both");

        // 1.5 s to 2.5 s: the video from the keyframe at 1 s, and the audio frames that start
        // from then to 2.5 s, which are the 47th (1.0027 s) to the 117th (2.4960 s).
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut(&whole, &cut, Duration::from_secs_f64(1.5), Duration::from_secs_f64(2.5)).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs_f64(2.5)));
        let part = Movie::open(&cut).unwrap();
        assert_eq!(part.frames(), 45);
        assert_eq!(part.sound.len(), 1);
        let audio = &part.sound[0];
        assert_eq!(audio.samples.len(), 71);
        assert_eq!((audio.id(), audio.timescale), (2, 48_000));
        assert_eq!(audio.samples[0].dts, 0);
        // Shown 2.7 ms after the video starts, as it was; the edit list says so to the ms.
        assert_eq!(audio.shift, 96, "2 ms, in audio units");
        assert!(audio.samples.iter().all(|s| s.keyframe));

        // About as long as the video: not before it ends, and no more than a frame after.
        let video_end = part.duration().as_secs_f64();
        let audio_end = audio.samples.last().map(|s| (audio.shown(s) + s.duration as i64) as f64 / 48_000.0).unwrap();
        assert!((video_end - 1.5).abs() < 1e-6);
        assert!(audio_end <= video_end + 1024.0 / 48_000.0 && audio_end > video_end - 1024.0 / 48_000.0, "{audio_end} against {video_end}");

        // The bytes are the same: the 30th to 74th frames, and the 47th to 117th audio frames.
        let file = std::fs::read(&cut).unwrap();
        for (n, bytes) in (30u8..).zip(bytes_of(&file, &part.video)) {
            assert!(bytes.iter().all(|&b| b == n) && bytes.len() == 10 + n as usize % 7, "frame {n}");
        }
        for (n, bytes) in (47u32..).zip(bytes_of(&file, audio)) {
            assert!(bytes.iter().all(|&b| b == audio_byte(n)) && bytes.len() == 20 + n as usize % 5, "audio frame {n}");
        }
        // The headers: the movie is as long as the longer track, in ms (the sound's 2 ms later
        // start, then its 1514.7 ms, rounded up); the audio's own in 48 kHz.
        assert_eq!(be32(full(&part.mvhd), 12), 1517, "mvhd, in ms");
        assert_eq!(be32(full(&audio.mdhd), 12), 71 * 1024, "mdhd, in audio units");
        assert_eq!(be32(full(&audio.tkhd), 16), 1517, "tkhd, in ms");
        assert_eq!(be32(full(&part.video.tkhd), 16), 1500);
        assert_eq!(be32(&part.mvhd, 104), 3);
        // The data of each second goes together: the first second's sound is before the second's
        // picture, and all of it is after the first picture.
        let first_second_of_video = part.video.samples[0].offset;
        let sound_at = audio.samples[0].offset;
        assert!(first_second_of_video < sound_at && sound_at < part.video.samples[30].offset, "interleaved");

        // Cut again, from the cut: still both, and with nothing cut it's all there.
        let again = dir.join("again.mp4");
        part.cut(&cut, &again, Duration::ZERO, Duration::MAX).unwrap();
        let again = Movie::open(&again).unwrap();
        assert_eq!((again.frames(), again.sound[0].samples.len()), (45, 71));
        let whole_again = dir.join("whole-again.mp4");
        read.cut(&whole, &whole_again, Duration::ZERO, Duration::MAX).unwrap();
        let all = Movie::open(&whole_again).unwrap();
        assert_eq!((all.frames(), all.sound[0].samples.len()), (90, 141));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each track's edit list in `file`, as its (length, media time) entries (version 0).
    fn edit_lists(file: &[u8]) -> Vec<Vec<(u32, i32)>> {
        let moov = children(file).find(|(k, _)| k == b"moov").map(|(_, b)| content(b)).unwrap();
        let elst = |trak: &[u8]| find(content(trak), b"edts").and_then(|e| find(content(e), b"elst")).map(|e| entries(full(e), 12).map(|e| (be32(e, 0), be32(e, 4) as i32)).collect());
        children(moov).filter(|(k, _)| k == b"trak").map(|(_, t)| elst(t).unwrap_or_default()).collect()
    }

    #[test]
    fn sound_that_starts_after_the_picture_still_does_when_cut() {
        let dir = temp_dir("late");
        // As mp4mux writes it: the sound shown 50 ms after the picture, by an empty edit.
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        movie.sound[0].shift = 2400;
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        // With nothing cut it's as it was: 50 ms of nothing, then the sound, as far as the picture
        // goes (the 139 audio frames that start in its 3 s, 2.965 s of them).
        assert_eq!(edit_lists(&std::fs::read(&whole).unwrap()), [vec![], vec![(50, -1), (2966, 0)]]);
        let read = Movie::open(&whole).unwrap();
        assert_eq!((read.video.shift, read.sound[0].shift, read.sound[0].samples.len()), (0, 2400, 139));
        assert_eq!(read.shown()[1][0], 0.05);
        assert_eq!(be32(full(&read.mvhd), 12), 3016, "the movie lasts until the sound ends, in ms");
        assert!((read.duration().as_secs_f64() - 3.0).abs() < 1e-6, "the picture's length");

        // 1.5 s to 2.5 s, from the keyframe at 1 s. The first audio frame shown from then is the
        // 45th, at 1.010 s (its media time is 0.960 s), so it's shown 10 ms in; the last is the
        // 114th.
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut(&whole, &cut, Duration::from_secs_f64(1.5), Duration::from_secs_f64(2.5)).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs_f64(2.5)));
        let file = std::fs::read(&cut).unwrap();
        assert_eq!(edit_lists(&file), [vec![], vec![(10, -1), (1494, 0)]]);
        let part = Movie::open(&cut).unwrap();
        let audio = &part.sound[0];
        assert_eq!((audio.shift, audio.samples.len()), (480, 70));
        for (n, bytes) in (45u32..).zip(bytes_of(&file, audio)) {
            assert!(bytes.iter().all(|&b| b == audio_byte(n)), "audio frame {n}");
        }
        // Each is shown as long after the cut's start as it was after the keyframe.
        let (before, after) = (read.shown(), part.shown());
        for (n, t) in (45..).zip(&after[1]) {
            assert!((t + 1.0 - before[1][n]).abs() < 1e-9, "audio frame {n} at {t}");
        }
        assert_eq!(after[0][0], 0.0, "the picture at the start");

        // A cut of the cut, with nothing cut, keeps it too.
        let again = dir.join("again.mp4");
        part.cut(&cut, &again, Duration::ZERO, Duration::MAX).unwrap();
        assert_eq!(Movie::open(&again).unwrap().shown(), after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_shown_later_than_its_decoded_is_cut_by_when_its_shown() {
        let dir = temp_dir("reordered");
        // Each frame shown two frames after it's decoded (as with B-frames), and the edit list
        // starting where the first is shown, at media time 6000.
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        movie.video.ctts_version = Some(0);
        for s in &mut movie.video.samples {
            s.cts = 6000;
        }
        movie.video.shift = -6000;
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        assert_eq!(edit_lists(&std::fs::read(&whole).unwrap()), [vec![(3000, 6000)], vec![]], "all 3 s, from media time 6000");
        let read = Movie::open(&whole).unwrap();
        assert_eq!(read.video.shift, -6000);
        assert_eq!(read.keyframes(), [0.0, 1.0, 2.0].map(Duration::from_secs_f64));
        assert!((read.duration().as_secs_f64() - 3.0).abs() < 1e-6);

        // From the keyframe shown at 1 s, which is shown first, with the sound of then.
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut(&whole, &cut, Duration::from_secs_f64(1.5), Duration::from_secs_f64(2.5)).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs_f64(2.5)));
        assert_eq!(edit_lists(&std::fs::read(&cut).unwrap()), [vec![(1500, 6000)], vec![(2, -1), (1515, 0)]]);
        let part = Movie::open(&cut).unwrap();
        assert_eq!((part.frames(), part.video.shift), (45, -6000));
        assert_eq!(part.keyframes(), [0.0, 1.0].map(Duration::from_secs_f64));
        assert!((part.duration().as_secs_f64() - 1.5).abs() < 1e-6);
        assert_eq!(part.sound[0].samples.len(), 71, "the 47th to the 117th, as without B-frames");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_of_just_the_video_leaves_the_sound_out_and_keeps_its_edit_list() {
        let dir = temp_dir("video-only");
        // Shown two frames after it's decoded, from media time 6000, with sound 50 ms late.
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        movie.video.ctts_version = Some(0);
        for s in &mut movie.video.samples {
            s.cts = 6000;
        }
        movie.video.shift = -6000;
        movie.sound[0].shift = 2400;
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        let read = Movie::open(&whole).unwrap();
        assert!(read.has_sound());

        // From the keyframe shown at 1 s, as a cut with the sound would.
        let cut = dir.join("cut.mp4");
        let (from, to) = read.cut_tracks(&whole, &cut, Duration::from_secs_f64(1.5), Duration::MAX, false).unwrap();
        assert_eq!((from, to), (Duration::from_secs(1), Duration::from_secs(3)));
        let file = std::fs::read(&cut).unwrap();
        assert_eq!(edit_lists(&file), [vec![(2000, 6000)]], "one track: 2 s from media time 6000");
        let part = Movie::open(&cut).unwrap();
        assert!(!part.has_sound());
        assert_eq!((part.frames(), part.video.shift, part.video.id()), (60, -6000, 1));
        assert_eq!(part.keyframes(), [0.0, 1.0].map(Duration::from_secs_f64));
        assert!((part.duration().as_secs_f64() - 2.0).abs() < 1e-6);
        // The 30th to the 89th frames, byte for byte, and nothing else in its data.
        for (n, bytes) in (30u8..).zip(bytes_of(&file, &part.video)) {
            assert!(bytes.iter().all(|&b| b == n) && bytes.len() == 10 + n as usize % 7, "frame {n}");
        }
        let video_bytes: u64 = part.video.samples.iter().map(|s| s.size as u64).sum();
        assert_eq!(file.len() as u64, part.video.samples[0].offset + video_bytes);
        assert_eq!(be32(full(&part.mvhd), 12), 2000, "mvhd, in ms: the picture's length, not the sound's");

        // The same as the picture of a cut with the sound.
        let both = dir.join("both.mp4");
        assert_eq!(read.cut(&whole, &both, Duration::from_secs_f64(1.5), Duration::MAX).unwrap(), (from, to));
        let both = Movie::open(&both).unwrap();
        assert!(both.has_sound());
        assert_eq!(both.shown()[0], part.shown()[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_keeps_the_frames_b_frames_are_drawn_from() {
        // Decoded in the order 0 3 1 2 6 4 5 (in frames shown): the B-frames come after the frame
        // after them, which is shown a frame late so that none is shown before it's decoded.
        let (mut movie, data) = movie(7, 30);
        movie.video.ctts_version = Some(0);
        for (s, shown) in movie.video.samples.iter_mut().zip([0u64, 3, 1, 2, 6, 4, 5]) {
            s.cts = ((shown + 1) * 3000) as i32 - s.dts as i32;
        }
        movie.video.shift = -3000;
        assert_eq!(movie.shown()[0], [0, 3, 1, 2, 6, 4, 5].map(|n| n as f64 / 30.0));
        // Up to the 4th frame shown: the 3rd needs the 4th, decoded before it.
        let to = |frames: u64| Duration::from_secs(frames) / 30;
        assert_eq!(movie.span(Duration::ZERO, to(3)), 0..4);
        assert_eq!(movie.span(Duration::ZERO, to(4)), 0..4);
        assert_eq!(movie.span(Duration::ZERO, to(5)), 0..6);
        assert_eq!(movie.span(Duration::ZERO, to(1)), 0..1);
        assert!((movie.duration().as_secs_f64() - 7.0 / 30.0).abs() < 1e-9);

        // Cut there, the 4th is kept but not shown: the edit list ends before it.
        let dir = temp_dir("b-frames");
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        let cut = dir.join("cut.mp4");
        let (_, end) = Movie::open(&whole).unwrap().cut(&whole, &cut, Duration::ZERO, to(3)).unwrap();
        assert_eq!(end, to(3));
        assert_eq!(edit_lists(&std::fs::read(&cut).unwrap()), [vec![(100, 3000)]], "100 ms, from media time 3000");
        let part = Movie::open(&cut).unwrap();
        assert_eq!((part.frames(), part.duration()), (4, to(3)));
        // And cut again with nothing cut, it still isn't.
        let again = dir.join("again.mp4");
        assert_eq!(part.cut(&cut, &again, Duration::ZERO, Duration::MAX).unwrap().1, to(3));
        assert_eq!(edit_lists(&std::fs::read(&again).unwrap()), [vec![(100, 3000)]]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_lists_say_how_much_later_a_track_is_shown() {
        let v0 = |edits: &[(u32, i32)]| {
            let entries: Vec<u8> = edits.iter().flat_map(|&(d, m)| [d.to_be_bytes(), m.to_be_bytes(), 0x0001_0000u32.to_be_bytes()].concat()).collect();
            full_box(b"elst", 0, &[&(edits.len() as u32).to_be_bytes()[..], &entries].concat())
        };
        // mp4mux's, for sound: 150 of 3000 a second of nothing (50 ms), then 9904 of it.
        assert_eq!(edit_list(&v0(&[(150, -1), (9904, 0)]), 3000, 48_000), (2400, Some(160_864)));
        assert_eq!(edit_list(&v0(&[(10_000, 0)]), 3000, 3000), (0, Some(10_000)), "and for the picture");
        assert_eq!(edit_list(&v0(&[(10_000, 200)]), 3000, 3000), (-200, Some(10_000)), "from media time 200");
        assert_eq!(edit_list(&v0(&[(30, -1), (30, -1), (10_000, 200)]), 3000, 3000), (-140, Some(10_060)));
        assert_eq!(edit_list(&v0(&[(100, 50), (100, 500)]), 1000, 1000), (-50, Some(100)), "only the first real edit counts");
        assert_eq!(edit_list(&v0(&[(0, 50)]), 1000, 1000), (-50, None), "0 is all of it");
        assert_eq!(edit_list(&v0(&[]), 1000, 1000), (0, None));
        assert_eq!(edit_list(&v0(&[(u32::MAX, -1)]), 1, 1 << 30), (1 << 40, None), "within bounds");
        // Version 1, with 64-bit times.
        let entry = |d: u64, m: i64| [&d.to_be_bytes()[..], &m.to_be_bytes(), &0x0001_0000u32.to_be_bytes()].concat();
        let v1 = full_box(b"elst", 1, &[&2u32.to_be_bytes()[..], &entry(150, -1), &entry(9904, 0)].concat());
        assert_eq!(edit_list(&v1, 3000, 48_000), (2400, Some(160_864)));
        // Cut short, it reads what's there.
        assert_eq!(edit_list(&v1[..v1.len() - 20], 3000, 48_000), (2400, None));
        assert_eq!(edit_list(&v1[..10], 3000, 48_000), (0, None));
    }

    #[test]
    fn a_cut_too_short_for_a_sound_frame_has_no_sound_track() {
        let dir = temp_dir("short");
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        // Sound that starts 2 s in, as if the file had it that way.
        let mut read = Movie::open(&whole).unwrap();
        for s in &mut read.sound[0].samples {
            s.dts += 96_000;
        }
        let cut = dir.join("cut.mp4");
        read.cut(&whole, &cut, Duration::ZERO, Duration::from_millis(500)).unwrap();
        let part = Movie::open(&cut).unwrap();
        assert!(!part.has_sound() && part.frames() == 15);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The offset of the `n`th box of `kind` in `file`.
    fn nth_box(file: &[u8], kind: &[u8; 4], n: usize) -> usize {
        let at: Vec<usize> = (4..file.len() - 4).filter(|&i| &file[i..i + 4] == kind).collect();
        at[n] - 4
    }

    #[test]
    fn a_broken_sound_track_is_left_out_not_the_whole_cut() {
        let dir = temp_dir("broken");
        let (mut movie, data) = movie_with_sound(90, 141, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        let good = std::fs::read(&whole).unwrap();
        assert!(Movie::open(&whole).unwrap().has_sound());

        let broken = dir.join("broken.mp4");
        let cut = dir.join("cut.mp4");
        let damage = |what: &dyn Fn(&mut Vec<u8>)| {
            let mut file = good.clone();
            what(&mut file);
            std::fs::write(&broken, &file).unwrap();
            let read = Movie::open(&broken).unwrap();
            assert!(!read.has_sound(), "the sound is left out");
            read.cut(&broken, &cut, Duration::from_secs(1), Duration::from_secs(2)).unwrap();
            let part = Movie::open(&cut).unwrap();
            assert!(!part.has_sound() && part.frames() == 30, "the picture is cut as usual");
            // Only the picture's data is in it, and its index says so.
        };
        // Its sample sizes are cut short (the second stsz is the sound's)...
        damage(&|file| {
            let at = nth_box(file, b"stsz", 1);
            file[at + 16..at + 20].copy_from_slice(&u32::MAX.to_be_bytes());
        });
        // ...its samples are past the end of the file...
        damage(&|file| {
            let at = nth_box(file, b"co64", 1);
            file[at + 16..at + 24].copy_from_slice(&(1u64 << 40).to_be_bytes());
        });
        // ...its timing is missing...
        damage(&|file| {
            let at = nth_box(file, b"stts", 1);
            file[at + 4..at + 8].copy_from_slice(b"free");
        });
        // ...it has the video's track number...
        damage(&|file| {
            let at = nth_box(file, b"tkhd", 1);
            file[at + 20..at + 24].copy_from_slice(&1u32.to_be_bytes());
        });
        // ...or it's the kind of track that isn't sound or picture.
        damage(&|file| {
            let at = nth_box(file, b"hdlr", 1);
            file[at + 16..at + 20].copy_from_slice(b"text");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_with_only_sound_is_refused() {
        let dir = temp_dir("audio-only");
        let (mut movie, data) = movie_with_sound(30, 47, 30);
        let whole = dir.join("whole.mp4");
        write(&mut movie, &data, &whole);
        let file = std::fs::read(&whole).unwrap();
        let moov = children(&file).find(|(k, _)| k == b"moov").map(|(_, b)| content(b)).unwrap();
        let traks: Vec<&[u8]> = children(moov).filter(|(k, _)| k == b"trak").map(|(_, b)| b).collect();
        assert_eq!(traks.len(), 2);
        let only_sound = [find(moov, b"mvhd").unwrap().to_vec(), traks[1].to_vec()].concat();
        let err = Movie::parse(Vec::new(), &only_sound).err().unwrap();
        assert_eq!(err, "Not an MP4 RSnip can read: no video");
        assert!(Movie::parse(Vec::new(), &[find(moov, b"mvhd").unwrap().to_vec(), traks[0].to_vec()].concat()).is_ok(), "and the picture alone is fine");
        let _ = std::fs::remove_dir_all(&dir);
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
        fn damaged_indexes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..400), cut in 0usize..400, sound in proptest::prelude::any::<bool>()) {
            let (mut movie, data) = if sound { movie_with_sound(40, 60, 10) } else { movie(40, 10) };
            let path = std::env::temp_dir().join(format!("rsnip-damaged-{}-{}-{}", std::process::id(), cut, sound));
            write(&mut movie, &data, &path);
            let mut file = std::fs::read(&path).unwrap();
            let at = cut.min(file.len());
            let end = (at + bytes.len()).min(file.len());
            file[at..end].copy_from_slice(&bytes[..end - at]);
            std::fs::write(&path, &file).unwrap();
            let copy = path.with_extension("copy");
            let _ = Movie::open(&path).map(|m| (m.span(Duration::from_secs(1), Duration::from_secs(2)), m.cut(&path, &copy, Duration::from_secs(1), Duration::from_secs(2))));
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&copy);
            let _ = std::fs::remove_file(path.with_extension("raw"));
        }
    }

    /// Whether `program` can be run.
    fn runs_here(program: &str) -> bool {
        std::process::Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
    }

    fn gst_inspect(element: &str) -> bool {
        std::process::Command::new("gst-inspect-1.0").arg(element).output().is_ok_and(|o| o.status.success())
    }

    /// A file GStreamer's mp4mux wrote, with H.264 and AAC, cut: both streams are still there, as
    /// long as each other, and it decodes. Skipped without GStreamer's x264 and an AAC encoder.
    #[test]
    fn a_cut_of_a_file_from_mp4mux_has_both_streams_and_plays() {
        let Some(aac) = ["fdkaacenc", "avenc_aac", "voaacenc"].into_iter().find(|e| gst_inspect(e)) else {
            return eprintln!("skipped: no AAC encoder");
        };
        if !runs_here("gst-launch-1.0") || !gst_inspect("x264enc") || !gst_inspect("mp4mux") || !gst_inspect("avdec_h264") {
            return eprintln!("skipped: no GStreamer x264enc, mp4mux and an H.264 decoder");
        }
        let dir = temp_dir("gst");
        let whole = dir.join("whole.mp4");
        // 3 s of video with a keyframe a second, and 3 s of sound.
        let pipeline = format!(
            "videotestsrc num-buffers=90 ! video/x-raw,width=160,height=120,framerate=30/1 ! x264enc key-int-max=30 ! h264parse ! mp4mux name=m ! filesink location={} \
             audiotestsrc num-buffers=141 samplesperbuffer=1024 ! audio/x-raw,rate=48000,channels=2 ! audioconvert ! {aac} ! m.",
            whole.display()
        );
        let made = std::process::Command::new("gst-launch-1.0").arg("-q").args(pipeline.split_whitespace()).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));

        let movie = Movie::open(&whole).unwrap();
        assert!(movie.has_sound(), "read back from the muxer's file");
        let cut = dir.join("cut.mp4");
        let (from, to) = movie.cut(&whole, &cut, Duration::from_millis(1500), Duration::from_millis(2500)).unwrap();
        assert_eq!(from, Duration::from_secs(1));
        assert!(to.abs_diff(Duration::from_millis(2500)) < Duration::from_millis(50), "{to:?}");

        let part = Movie::open(&cut).unwrap();
        assert!(part.has_sound());
        let seconds = |track: &Track| track.samples.iter().map(|s| s.duration as f64).sum::<f64>() / track.timescale as f64;
        assert!((seconds(&part.video) - 1.5).abs() < 0.05, "{}", seconds(&part.video));
        assert!((seconds(&part.sound[0]) - seconds(&part.video)).abs() < 0.05, "{} and {}", seconds(&part.sound[0]), seconds(&part.video));

        // Both streams decode (-v says what comes out of each).
        let played = std::process::Command::new("gst-launch-1.0")
            .args(["-v", "filesrc"])
            .arg(format!("location={}", cut.display()))
            .args(["!", "decodebin", "name=d", "d.", "!", "queue", "!", "fakesink", "d.", "!", "queue", "!", "fakesink"])
            .output()
            .unwrap();
        let (said, errors) = (String::from_utf8_lossy(&played.stdout), String::from_utf8_lossy(&played.stderr));
        assert!(played.status.success() && !said.contains("ERROR") && errors.is_empty(), "{said}{errors}");
        assert!(said.contains("audio/x-raw") && said.contains("video/x-raw"), "{said}");
        // And GStreamer's own tool, where there is one, finds the same two.
        if let Ok(found) = std::process::Command::new("gst-discoverer-1.0").arg(&cut).output() {
            let found = String::from_utf8_lossy(&found.stdout);
            assert!(found.contains("audio #") && found.contains("video #"), "{found}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
