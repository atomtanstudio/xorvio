//! What a live stream holds, learned from the first bytes the gateway reads
//! before ffmpeg starts on it: an MPEG-TS stream's PAT and PMT, an FLV
//! stream's first tags, an HLS variant's CODECS attribute or an fMP4 init
//! section. The live buffer's ffmpeg arguments depend on it (copy what the
//! web view plays, convert the rest), and it has to come from the one
//! connection the provider allows, so ffprobe can't be asked first.

use std::collections::HashMap;

/// An MPEG-TS packet.
pub(crate) const PACKET: usize = 188;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Video {
    H264,
    Hevc,
    /// MPEG-1 or MPEG-2 video, still common on broadcast feeds.
    Mpeg2,
    /// Anything else (VC-1, MPEG-4 Part 2, AV1…) or not known.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Audio {
    /// AAC with ADTS headers, as MPEG-TS carries it.
    Aac,
    /// AAC in LATM, used by DVB-T2; nothing on the web decodes it.
    AacLatm,
    Ac3,
    Eac3,
    /// MPEG-1/2 audio: MP2 on broadcast feeds, sometimes MP3.
    Mpeg,
    /// Anything else (DTS, Opus…) or not known.
    Other,
}

impl Video {
    /// For the player's stream information; None when it isn't known.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Video::H264 => Some("H.264"),
            Video::Hevc => Some("HEVC"),
            Video::Mpeg2 => Some("MPEG-2"),
            Video::Other => None,
        }
    }
}

impl Audio {
    /// For the player's stream information; None when it isn't known.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Audio::Aac => Some("AAC"),
            Audio::AacLatm => Some("AAC (LATM)"),
            Audio::Ac3 => Some("AC-3"),
            Audio::Eac3 => Some("E-AC-3"),
            Audio::Mpeg => Some("MPEG audio"),
            Audio::Other => None,
        }
    }
}

/// One elementary stream: its codec and, in MPEG-TS, its PID (which ffmpeg
/// uses as the stream's id, so it can be mapped exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Track<C> {
    pub codec: C,
    pub pid: Option<u16>,
}

/// The first video and first audio stream, the only ones the buffer keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Codecs {
    pub video: Option<Track<Video>>,
    pub audio: Option<Track<Audio>>,
}

impl Codecs {
    /// Nothing could be read: assume both, and convert both.
    pub fn unknown() -> Codecs {
        Codecs {
            video: Some(Track { codec: Video::Other, pid: None }),
            audio: Some(Track { codec: Audio::Other, pid: None }),
        }
    }

    /// The same without PIDs, for an input whose streams ffmpeg numbers
    /// itself (HLS).
    pub fn without_pids(self) -> Codecs {
        Codecs {
            video: self.video.map(|t| Track { pid: None, ..t }),
            audio: self.audio.map(|t| Track { pid: None, ..t }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.video.is_none() && self.audio.is_none()
    }

    fn of(streams: &[(u16, Kind)]) -> Codecs {
        Codecs {
            video: streams.iter().find_map(|&(pid, kind)| match kind {
                Kind::Video(codec) => Some(Track { codec, pid: Some(pid) }),
                _ => None,
            }),
            audio: streams.iter().find_map(|&(pid, kind)| match kind {
                Kind::Audio(codec) => Some(Track { codec, pid: Some(pid) }),
                _ => None,
            }),
        }
    }
}

/// What a PMT entry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Video(Video),
    Audio(Audio),
    /// Teletext, subtitles, data: left out.
    Other,
}

/// A PMT entry by its stream type and descriptors. Private-data streams
/// (0x06) are told apart by their DVB descriptors or a registration code;
/// anything that might be audio counts as audio, so the first audio here is
/// the first ffmpeg sees too.
pub(crate) fn classify(stream_type: u8, descriptors: &[u8]) -> Kind {
    match stream_type {
        0x1B => Kind::Video(Video::H264),
        0x24 => Kind::Video(Video::Hevc),
        0x01 | 0x02 => Kind::Video(Video::Mpeg2),
        // MPEG-4 Part 2, CAVS, VC-1, VVC.
        0x10 | 0x42 | 0xEA | 0x33 => Kind::Video(Video::Other),
        0x0F => Kind::Audio(Audio::Aac),
        0x11 => Kind::Audio(Audio::AacLatm),
        0x03 | 0x04 => Kind::Audio(Audio::Mpeg),
        0x81 => Kind::Audio(Audio::Ac3),
        0x87 => Kind::Audio(Audio::Eac3),
        // DTS in its ATSC and Blu-ray guises.
        0x82 | 0x85 | 0x86 | 0x8A => Kind::Audio(Audio::Other),
        0x06 => private_stream(descriptors),
        _ => Kind::Other,
    }
}

fn private_stream(descriptors: &[u8]) -> Kind {
    for (tag, data) in descriptor_list(descriptors) {
        match tag {
            0x6A => return Kind::Audio(Audio::Ac3),
            0x7A => return Kind::Audio(Audio::Eac3),
            // DTS, and AAC whose packaging (ADTS or LATM) it doesn't say.
            0x7B | 0x7C => return Kind::Audio(Audio::Other),
            // Extension descriptors: DTS-HD, AC-4.
            0x7F if matches!(data.first(), Some(0x0E | 0x15)) => return Kind::Audio(Audio::Other),
            0x05 => match data.get(..4) {
                Some(b"AC-3") => return Kind::Audio(Audio::Ac3),
                Some(b"EAC3") => return Kind::Audio(Audio::Eac3),
                Some(b"DTS1" | b"DTS2" | b"DTS3" | b"Opus" | b"BSSD") => return Kind::Audio(Audio::Other),
                Some(b"HEVC") => return Kind::Video(Video::Hevc),
                _ => {}
            },
            _ => {}
        }
    }
    // Teletext (0x56), DVB subtitles (0x59) and anything unknown.
    Kind::Other
}

fn descriptor_list(mut bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    std::iter::from_fn(move || {
        let (&tag, rest) = bytes.split_first()?;
        let (&length, rest) = rest.split_first()?;
        let data = rest.get(..length as usize)?;
        bytes = &rest[length as usize..];
        Some((tag, data))
    })
}

/// Reads an MPEG-TS stream as it arrives: the PAT, the PMT it points at,
/// and where the video can first be decoded from (a packet flagged as a
/// random access point, or one whose picture starts with parameter sets).
/// The buffer is fed from there, so ffmpeg finds the parameter sets at
/// once and starts in a couple of seconds instead of probing for five.
#[derive(Debug, Default)]
pub(crate) struct TsScan {
    /// The first packet's offset: a sync byte with two more a packet apart.
    sync: Option<usize>,
    /// The next packet to read.
    next: usize,
    pmt_pid: Option<u16>,
    /// PSI sections being put together, by PID, with the packets they came in.
    partial: HashMap<u16, (Vec<u8>, Vec<usize>)>,
    pat_packets: Vec<usize>,
    pmt_packets: Vec<usize>,
    streams: Option<Vec<(u16, Kind)>>,
    /// The first random access point of the video.
    random_access: Option<usize>,
    /// Where the search for it goes on from (it starts again from the first
    /// packet once the PMT says which PID the video is).
    search: usize,
}

impl TsScan {
    /// Reads the packets in `buf` (everything received so far, which only
    /// ever grows) that haven't been read yet.
    pub fn scan(&mut self, buf: &[u8]) {
        if self.sync.is_none() {
            let Some(sync) = find_sync(buf) else { return };
            self.sync = Some(sync);
            self.next = sync;
            self.search = sync;
        }
        while self.next + PACKET <= buf.len() {
            let packet = &buf[self.next..self.next + PACKET];
            if packet[0] != 0x47 {
                // Lost sync (a broken packet): find it again further on.
                match resync(&buf[self.next..]) {
                    Some(skip) => {
                        self.next += skip;
                        continue;
                    }
                    None => break,
                }
            }
            let offset = self.next;
            self.next += PACKET;
            let Some((pid, unit_start, payload)) = split(packet) else { continue };
            if pid == 0 || Some(pid) == self.pmt_pid {
                self.psi(pid, unit_start, payload, offset);
            }
        }
        self.find_random_access(buf);
    }

    /// Whether this is MPEG-TS at all (as far as the bytes so far show).
    pub fn is_transport(&self) -> bool {
        self.sync.is_some()
    }

    /// The program's streams are known.
    pub fn knows_program(&self) -> bool {
        self.streams.is_some()
    }

    /// The program is known and the video (if any) can be decoded from a
    /// known point: nothing more to wait for.
    pub fn is_ready(&self) -> bool {
        self.knows_program() && (self.random_access.is_some() || self.codecs().is_some_and(|c| c.video.is_none()))
    }

    pub fn codecs(&self) -> Option<Codecs> {
        self.streams.as_deref().map(Codecs::of)
    }

    pub fn starts_at_random_access(&self) -> bool {
        self.random_access.is_some()
    }

    /// What to give ffmpeg first: the PAT and PMT, then everything from the
    /// video's first random access point; or all of it from the first
    /// packet when that point isn't known.
    pub fn start_bytes(&self, buf: &[u8]) -> Vec<u8> {
        let Some(sync) = self.sync else { return buf.to_vec() };
        let Some(from) = self.random_access else { return buf[sync..].to_vec() };
        let mut out = Vec::with_capacity(buf.len() - from + PACKET * 4);
        for &offset in self.pat_packets.iter().chain(&self.pmt_packets) {
            out.extend_from_slice(&buf[offset..offset + PACKET]);
        }
        out.extend_from_slice(&buf[from..]);
        out
    }

    fn psi(&mut self, pid: u16, unit_start: bool, payload: &[u8], offset: usize) {
        if unit_start {
            let Some((&pointer, rest)) = payload.split_first() else { return };
            let Some(section) = rest.get(pointer as usize..) else { return };
            self.partial.insert(pid, (section.to_vec(), vec![offset]));
        } else if let Some((bytes, packets)) = self.partial.get_mut(&pid) {
            bytes.extend_from_slice(payload);
            packets.push(offset);
        } else {
            return;
        }
        let Some((bytes, _)) = self.partial.get(&pid) else { return };
        if bytes.len() < 3 {
            return;
        }
        let length = 3 + ((usize::from(bytes[1] & 0x0F) << 8) | usize::from(bytes[2]));
        if bytes.len() < length {
            return;
        }
        let Some((bytes, packets)) = self.partial.remove(&pid) else { return };
        let section = &bytes[..length];
        if length < 12 || crc32(section) != 0 {
            return;
        }
        match section[0] {
            0x00 if pid == 0 && self.pmt_pid.is_none() => {
                // The first program's PMT (program 0 points at the network table).
                let entries = &section[8..length - 4];
                if let Some(entry) = entries.as_chunks::<4>().0.iter().find(|e| u16::from_be_bytes([e[0], e[1]]) != 0) {
                    self.pmt_pid = Some((u16::from(entry[2] & 0x1F) << 8) | u16::from(entry[3]));
                    self.pat_packets = packets;
                }
            }
            0x02 if Some(pid) == self.pmt_pid && self.streams.is_none() => {
                if let Some(streams) = parse_pmt(section) {
                    self.streams = Some(streams);
                    self.pmt_packets = packets;
                }
            }
            _ => {}
        }
    }

    fn find_random_access(&mut self, buf: &[u8]) {
        if self.random_access.is_some() {
            return;
        }
        let Some(video) = self.codecs().and_then(|c| c.video) else { return };
        let Some(pid) = video.pid else { return };
        while self.search + PACKET <= self.next {
            let offset = self.search;
            self.search += PACKET;
            let packet = &buf[offset..offset + PACKET];
            if packet[0] != 0x47 {
                match resync(&buf[offset..self.next]) {
                    Some(skip) => self.search = offset + skip,
                    None => return,
                }
                continue;
            }
            if split(packet).is_some_and(|(p, _, _)| p == pid) && is_random_access(packet, video.codec) {
                self.random_access = Some(offset);
                return;
            }
        }
    }
}

/// The first offset where three packets in a row start with the sync byte.
fn find_sync(buf: &[u8]) -> Option<usize> {
    (0..PACKET.min(buf.len())).find(|&at| (0..3).all(|n| buf.get(at + n * PACKET) == Some(&0x47)))
}

/// After a broken packet, the next offset (past the first byte) where a whole
/// packet starts with the sync byte and so do the next two, as far as `buf`
/// goes. Garbage longer than a packet is skipped a packet at a time.
fn resync(buf: &[u8]) -> Option<usize> {
    let found = (1..PACKET.min(buf.len())).find(|&at| {
        at + PACKET <= buf.len() && (0..3).map(|n| at + n * PACKET).take_while(|&p| p < buf.len()).all(|p| buf[p] == 0x47)
    });
    match found {
        Some(at) => Some(at),
        None if buf.len() >= 4 * PACKET => Some(PACKET),
        None => None,
    }
}

/// A packet's PID, whether a unit (PES or section) starts in it, and its payload.
fn split(packet: &[u8]) -> Option<(u16, bool, &[u8])> {
    let pid = (u16::from(packet[1] & 0x1F) << 8) | u16::from(packet[2]);
    let unit_start = packet[1] & 0x40 != 0;
    let control = (packet[3] >> 4) & 0x03;
    let start = match control {
        0b01 => 4,
        0b11 => 5 + usize::from(packet[4]),
        _ => return Some((pid, unit_start, &[])),
    };
    Some((pid, unit_start, packet.get(start..).unwrap_or_default()))
}

/// Whether video can be decoded from this packet on: the random access flag
/// in its adaptation field, or a picture starting here with the parameter
/// sets (or keyframe) in its first bytes.
fn is_random_access(packet: &[u8], codec: Video) -> bool {
    let control = (packet[3] >> 4) & 0x03;
    if control & 0b10 != 0 && packet[4] > 0 && packet[5] & 0x40 != 0 {
        return true;
    }
    let Some((_, true, pes)) = split(packet) else { return false };
    if pes.len() < 9 || pes[..3] != [0, 0, 1] {
        return false;
    }
    let es = pes.get(9 + usize::from(pes[8])..).unwrap_or_default();
    es.windows(4).any(|w| {
        w[..3] == [0, 0, 1]
            && match codec {
                // IDR picture or sequence parameter set.
                Video::H264 => matches!(w[3] & 0x1F, 5 | 7),
                // IRAP pictures, video and sequence parameter sets.
                Video::Hevc => matches!((w[3] >> 1) & 0x3F, 16..=21 | 32 | 33),
                // Sequence header.
                Video::Mpeg2 => w[3] == 0xB3,
                Video::Other => false,
            }
    })
}

fn parse_pmt(section: &[u8]) -> Option<Vec<(u16, Kind)>> {
    let end = section.len().checked_sub(4)?;
    let program_info = (usize::from(section.get(10)? & 0x0F) << 8) | usize::from(*section.get(11)?);
    let mut at = 12 + program_info;
    let mut streams = Vec::new();
    while at + 5 <= end {
        let stream_type = section[at];
        let pid = (u16::from(section[at + 1] & 0x1F) << 8) | u16::from(section[at + 2]);
        let info = (usize::from(section[at + 3] & 0x0F) << 8) | usize::from(section[at + 4]);
        let descriptors = section.get(at + 5..(at + 5 + info).min(end))?;
        streams.push((pid, classify(stream_type, descriptors)));
        at += 5 + info;
    }
    Some(streams)
}

/// MPEG-2's CRC-32; zero over a whole section (its CRC included) when it's intact.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

/// What an FLV stream's header announces and its first tags show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FlvScan {
    has_video: bool,
    has_audio: bool,
    video: Option<Video>,
    audio: Option<Audio>,
}

impl FlvScan {
    /// Every stream the header announces has shown its codec.
    pub fn is_complete(&self) -> bool {
        (!self.has_video || self.video.is_some()) && (!self.has_audio || self.audio.is_some())
    }

    /// What's known; an announced stream not seen yet is converted to be safe.
    pub fn codecs(&self) -> Codecs {
        Codecs {
            video: (self.has_video || self.video.is_some())
                .then(|| Track { codec: self.video.unwrap_or(Video::Other), pid: None }),
            audio: (self.has_audio || self.audio.is_some())
                .then(|| Track { codec: self.audio.unwrap_or(Audio::Other), pid: None }),
        }
    }
}

/// Reads an FLV stream's header and as many whole tags as `buf` holds.
/// None when it isn't FLV.
pub(crate) fn scan_flv(buf: &[u8]) -> Option<FlvScan> {
    if buf.len() < 9 || &buf[..3] != b"FLV" {
        return None;
    }
    let mut scan = FlvScan {
        has_video: buf[4] & 0x01 != 0,
        has_audio: buf[4] & 0x04 != 0,
        ..FlvScan::default()
    };
    // The header, then the size of the (missing) tag before the first.
    let mut at = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize + 4;
    while let Some(header) = buf.get(at..at + 11) {
        let size = (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        let Some(data) = buf.get(at + 11..at + 11 + size) else { break };
        match header[0] & 0x1F {
            8 if scan.audio.is_none() => scan.audio = flv_audio(data),
            9 if scan.video.is_none() => scan.video = flv_video(data),
            _ => {}
        }
        at += 11 + size + 4;
    }
    Some(scan)
}

fn flv_video(data: &[u8]) -> Option<Video> {
    let first = *data.first()?;
    // Enhanced RTMP: a FourCC follows the frame type.
    if first & 0x80 != 0 {
        return Some(match data.get(1..5)? {
            b"avc1" => Video::H264,
            b"hvc1" => Video::Hevc,
            _ => Video::Other,
        });
    }
    Some(match first & 0x0F {
        7 => Video::H264,
        // The HEVC id Chinese CDNs use.
        12 => Video::Hevc,
        _ => Video::Other,
    })
}

fn flv_audio(data: &[u8]) -> Option<Audio> {
    Some(match data.first()? >> 4 {
        // AAC in FLV is raw with its config up front, so it copies like ADTS.
        10 => Audio::Aac,
        2 | 14 => Audio::Mpeg,
        // Enhanced RTMP: a FourCC follows.
        9 => match data.get(1..5)? {
            b"mp4a" => Audio::Aac,
            b"ac-3" => Audio::Ac3,
            b"ec-3" => Audio::Eac3,
            b".mp3" => Audio::Mpeg,
            _ => Audio::Other,
        },
        _ => Audio::Other,
    })
}

/// An HLS variant's CODECS attribute ("avc1.64001f,mp4a.40.2").
pub(crate) fn codecs_attribute(value: &str) -> Codecs {
    let mut codecs = Codecs::default();
    for entry in value.split(',') {
        let entry = entry.trim().to_ascii_lowercase();
        let fourcc = entry.split('.').next().unwrap_or_default();
        let video = match fourcc {
            "avc1" | "avc3" => Some(Video::H264),
            "hvc1" | "hev1" | "dvh1" | "dvhe" => Some(Video::Hevc),
            "mp4v" | "av01" | "vp09" | "vp08" | "vvc1" | "vvi1" => Some(Video::Other),
            _ => None,
        };
        let audio = match fourcc {
            "mp4a" if matches!(entry.as_str(), "mp4a.40.34" | "mp4a.69" | "mp4a.6b") => Some(Audio::Mpeg),
            "mp4a" => Some(Audio::Aac),
            "ac-3" => Some(Audio::Ac3),
            "ec-3" => Some(Audio::Eac3),
            "opus" | "flac" | "fl ac" | "ac-4" | "dtsc" | "dtse" | "dtsh" | "dtsx" | "mp3" => Some(Audio::Other),
            _ => None,
        };
        if codecs.video.is_none() {
            codecs.video = video.map(|codec| Track { codec, pid: None });
        }
        if codecs.audio.is_none() {
            codecs.audio = audio.map(|codec| Track { codec, pid: None });
        }
    }
    codecs
}

/// The codecs an fMP4 init section's sample entries name. None when it has none.
pub(crate) fn mp4_codecs(init: &[u8]) -> Option<Codecs> {
    let mut entries = Vec::new();
    sample_entries(init, &mut entries);
    let mut codecs = Codecs::default();
    for entry in entries {
        let video = match &entry {
            b"avc1" | b"avc3" => Some(Video::H264),
            b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => Some(Video::Hevc),
            b"mp4v" | b"av01" | b"vp09" | b"encv" => Some(Video::Other),
            _ => None,
        };
        let audio = match &entry {
            b"mp4a" => Some(Audio::Aac),
            b"ac-3" => Some(Audio::Ac3),
            b"ec-3" => Some(Audio::Eac3),
            b"Opus" | b"fLaC" | b"enca" | b"dtsc" | b"ac-4" => Some(Audio::Other),
            _ => None,
        };
        if codecs.video.is_none() {
            codecs.video = video.map(|codec| Track { codec, pid: None });
        }
        if codecs.audio.is_none() {
            codecs.audio = audio.map(|codec| Track { codec, pid: None });
        }
    }
    (!codecs.is_empty()).then_some(codecs)
}

fn sample_entries(bytes: &[u8], out: &mut Vec<[u8; 4]>) {
    for (kind, body) in boxes(bytes) {
        match &kind {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => sample_entries(body, out),
            // Version and flags, the entry count, then the entries.
            b"stsd" => out.extend(boxes(body.get(8..).unwrap_or_default()).map(|(kind, _)| kind)),
            _ => {}
        }
    }
}

/// The ISO-BMFF boxes in `bytes`, as (type, body).
fn boxes(mut bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let size = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let kind: [u8; 4] = bytes.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            0 => (8, bytes.len()),
            1 => (16, u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?) as usize),
            size => (8, size),
        };
        if size < header {
            return None;
        }
        let body = bytes.get(header..size).unwrap_or(&bytes[header.min(bytes.len())..]);
        bytes = bytes.get(size..).unwrap_or_default();
        Some((kind, body))
    })
}

/// A variant in a master playlist.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub codecs: Option<String>,
    /// Its audio rendition group, when it names one.
    pub audio: Option<String>,
}

/// The variants a master playlist lists (none for a media playlist).
pub(crate) fn variants(playlist: &str) -> Vec<Variant> {
    let mut found = Vec::new();
    let mut pending: Option<Variant> = None;
    for line in playlist.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attributes = attribute_list(attributes);
            pending = Some(Variant {
                uri: String::new(),
                bandwidth: attributes.get("BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                codecs: attributes.get("CODECS").map(|c| c.to_string()),
                audio: attributes.get("AUDIO").map(|a| a.to_string()),
            });
        } else if !line.starts_with('#')
            && let Some(variant) = pending.take()
        {
            found.push(Variant { uri: line.to_string(), ..variant });
        }
    }
    found
}

/// Whether an audio rendition group in a master playlist has playlists of
/// its own (rather than describing the audio inside the variants).
pub(crate) fn has_separate_audio(playlist: &str, group: &str) -> bool {
    playlist.lines().filter_map(|l| l.trim().strip_prefix("#EXT-X-MEDIA:")).any(|line| {
        let attributes = attribute_list(line);
        attributes.get("TYPE") == Some(&"AUDIO") && attributes.get("GROUP-ID") == Some(&group) && attributes.contains_key("URI")
    })
}

/// A media playlist's init section (fMP4) and its newest segment.
pub(crate) fn media_parts(playlist: &str) -> (Option<String>, Option<String>) {
    let mut init = None;
    let mut last = None;
    for line in playlist.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(attributes) = line.strip_prefix("#EXT-X-MAP:") {
            init = attribute_list(attributes).get("URI").map(|u| u.to_string());
        } else if !line.starts_with('#') {
            last = Some(line.to_string());
        }
    }
    (init, last)
}

/// `NAME=VALUE,NAME="VALUE, WITH COMMAS"` into a map, quotes removed.
fn attribute_list(text: &str) -> HashMap<&str, &str> {
    let mut attributes = HashMap::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some((name, after)) = rest.split_once('=') else { break };
        let (value, after) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.split_once('"') {
                Some((value, after)) => (value, after),
                None => (quoted, ""),
            },
            None => after.split_once(',').map_or((after, ""), |(value, after)| (value, after)),
        };
        attributes.insert(name.trim(), value);
        rest = after.trim_start_matches(',').trim_start();
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PSI section with its header, `body` and CRC, as the packets that carry it.
    fn section_packets(pid: u16, table_id: u8, body: &[u8], continuity: &mut u8) -> Vec<u8> {
        let mut section = vec![table_id, 0, 0, 0x00, 0x01, 0xC1, 0x00, 0x00];
        section.extend_from_slice(body);
        let length = section.len() - 3 + 4;
        section[1] = 0xB0 | (length >> 8) as u8;
        section[2] = length as u8;
        let crc = crc32(&section);
        section.extend_from_slice(&crc.to_be_bytes());
        let mut payload = vec![0u8];
        payload.extend_from_slice(&section);
        let mut packets = Vec::new();
        for (n, chunk) in payload.chunks(PACKET - 4).enumerate() {
            let mut packet = vec![0x47, (pid >> 8) as u8 & 0x1F, pid as u8, 0x10 | (*continuity & 0x0F)];
            if n == 0 {
                packet[1] |= 0x40;
            }
            *continuity += 1;
            packet.extend_from_slice(chunk);
            packet.resize(PACKET, 0xFF);
            packets.extend(packet);
        }
        packets
    }

    fn pat(pmt_pid: u16) -> Vec<u8> {
        // The network table first (program 0), then program 1.
        let body = [0x00, 0x00, 0xE0, 0x10, 0x00, 0x01, 0xE0 | (pmt_pid >> 8) as u8, pmt_pid as u8];
        section_packets(0, 0x00, &body, &mut 0)
    }

    fn pmt(pmt_pid: u16, streams: &[(u8, u16, &[u8])]) -> Vec<u8> {
        let mut body = vec![0xE1, 0x00, 0xF0, 0x00];
        for &(stream_type, pid, descriptors) in streams {
            body.extend([stream_type, 0xE0 | (pid >> 8) as u8, pid as u8, 0xF0, descriptors.len() as u8]);
            body.extend_from_slice(descriptors);
        }
        section_packets(pmt_pid, 0x02, &body, &mut 0)
    }

    /// A PES packet starting with `es` (and padding), optionally flagged as a random access point.
    fn pes_packet(pid: u16, es: &[u8], flagged: bool) -> Vec<u8> {
        let mut packet = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8];
        if flagged {
            packet.extend([0x30, 0x01, 0x40]);
        } else {
            packet.push(0x10);
        }
        packet.extend([0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 0x05, 0x21, 0, 1, 0, 1]);
        packet.extend_from_slice(es);
        packet.resize(PACKET, 0xFF);
        packet
    }

    fn continuation(pid: u16) -> Vec<u8> {
        let mut packet = vec![0x47, (pid >> 8) as u8, pid as u8, 0x10];
        packet.resize(PACKET, 0xAB);
        packet
    }

    /// Null packets, which streams are padded with.
    fn nulls(count: usize) -> Vec<u8> {
        let mut packet = vec![0x47, 0x1F, 0xFF, 0x10];
        packet.resize(PACKET, 0xFF);
        packet.repeat(count)
    }

    fn scan(buf: &[u8]) -> TsScan {
        let mut scan = TsScan::default();
        scan.scan(buf);
        scan
    }

    #[test]
    fn reads_h264_and_aac() {
        let mut stream = vec![0x12, 0x34, 0x56]; // a few bytes before the first packet
        stream.extend(pat(0x1000));
        stream.extend(pmt(0x1000, &[(0x1B, 0x100, &[]), (0x0F, 0x101, &[])]));
        stream.extend(nulls(2));
        let scan = scan(&stream);
        assert!(scan.is_transport() && scan.knows_program());
        assert_eq!(
            scan.codecs(),
            Some(Codecs {
                video: Some(Track { codec: Video::H264, pid: Some(0x100) }),
                audio: Some(Track { codec: Audio::Aac, pid: Some(0x101) }),
            })
        );
        assert!(!scan.is_ready(), "no random access point yet");
    }

    #[test]
    fn reads_dvb_descriptors_and_skips_teletext() {
        let teletext: &[u8] = &[0x56, 0x05, b'e', b'n', b'g', 0x09, 0x00];
        let ac3: &[u8] = &[0x0A, 0x04, b'e', b'n', b'g', 0x00, 0x6A, 0x01, 0x00];
        let eac3: &[u8] = &[0x7A, 0x01, 0x00];
        let mut stream = pat(0x20);
        stream.extend(pmt(0x20, &[(0x24, 0x200, &[]), (0x06, 0x201, teletext), (0x06, 0x202, ac3), (0x06, 0x203, eac3)]));
        stream.extend(nulls(2));
        let codecs = scan(&stream).codecs().unwrap();
        assert_eq!(codecs.video, Some(Track { codec: Video::Hevc, pid: Some(0x200) }));
        assert_eq!(codecs.audio, Some(Track { codec: Audio::Ac3, pid: Some(0x202) }), "teletext isn't audio");
    }

    #[test]
    fn reads_other_stream_types() {
        let kinds = |stream_type: u8, descriptors: &[u8]| classify(stream_type, descriptors);
        assert_eq!(kinds(0x02, &[]), Kind::Video(Video::Mpeg2));
        assert_eq!(kinds(0x01, &[]), Kind::Video(Video::Mpeg2));
        assert_eq!(kinds(0xEA, &[]), Kind::Video(Video::Other));
        assert_eq!(kinds(0x03, &[]), Kind::Audio(Audio::Mpeg));
        assert_eq!(kinds(0x04, &[]), Kind::Audio(Audio::Mpeg));
        assert_eq!(kinds(0x11, &[]), Kind::Audio(Audio::AacLatm));
        assert_eq!(kinds(0x81, &[]), Kind::Audio(Audio::Ac3));
        assert_eq!(kinds(0x87, &[]), Kind::Audio(Audio::Eac3));
        assert_eq!(kinds(0x06, &[0x05, 0x04, b'A', b'C', b'-', b'3']), Kind::Audio(Audio::Ac3));
        assert_eq!(kinds(0x06, &[0x05, 0x04, b'E', b'A', b'C', b'3']), Kind::Audio(Audio::Eac3));
        assert_eq!(kinds(0x06, &[0x7B, 0x00]), Kind::Audio(Audio::Other), "DTS");
        assert_eq!(kinds(0x06, &[0x59, 0x00]), Kind::Other, "DVB subtitles");
        assert_eq!(kinds(0x06, &[]), Kind::Other);
        assert_eq!(kinds(0x86, &[]), Kind::Audio(Audio::Other));
        assert_eq!(kinds(0x15, &[]), Kind::Other, "ID3 metadata");
    }

    #[test]
    fn finds_audio_only_programs_ready_at_once() {
        let mut stream = pat(0x30);
        stream.extend(pmt(0x30, &[(0x04, 0x300, &[])]));
        stream.extend(nulls(2));
        let scan = scan(&stream);
        assert_eq!(scan.codecs().unwrap().video, None);
        assert!(scan.is_ready(), "radio has no picture to wait for");
    }

    #[test]
    fn starts_at_the_first_random_access_point() {
        let mut stream = Vec::new();
        // A keyframe before the PMT is known still counts…
        stream.extend(continuation(0x100));
        stream.extend(pes_packet(0x101, &[0xFF, 0xF1], false));
        stream.extend(pes_packet(0x100, &[0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x67, 0x64], false));
        let keyframe = stream.len() - PACKET;
        stream.extend(continuation(0x100));
        stream.extend(pat(0x1000));
        stream.extend(pmt(0x1000, &[(0x1B, 0x100, &[]), (0x0F, 0x101, &[])]));
        let scan = scan(&stream);
        assert!(scan.is_ready() && scan.starts_at_random_access());
        let start = scan.start_bytes(&stream);
        assert_eq!(start.len(), 2 * PACKET + stream.len() - keyframe, "PAT and PMT, then from the keyframe on");
        assert_eq!(&start[2 * PACKET..], &stream[keyframe..]);
        assert_eq!(split(&start[..PACKET]).unwrap().0, 0, "the PAT comes first");
        assert_eq!(split(&start[PACKET..2 * PACKET]).unwrap().0, 0x1000);
    }

    #[test]
    fn spots_random_access_points() {
        let flagged = pes_packet(0x100, &[], true);
        assert!(is_random_access(&flagged, Video::Other), "the adaptation field's flag counts for any codec");
        let h264_slice = pes_packet(0x100, &[0, 0, 1, 0x41, 0x9A], false);
        assert!(!is_random_access(&h264_slice, Video::H264));
        assert!(is_random_access(&pes_packet(0x100, &[0, 0, 1, 0x65], false), Video::H264));
        // The same byte is an SEI in H.264 but an IDR picture in HEVC; the codec decides.
        assert!(!is_random_access(&pes_packet(0x100, &[0, 0, 1, 0x26, 0x01], false), Video::H264));
        assert!(is_random_access(&pes_packet(0x100, &[0, 0, 1, 0x40, 0x01], false), Video::Hevc), "VPS");
        assert!(is_random_access(&pes_packet(0x100, &[0, 0, 1, 0x26, 0x01], false), Video::Hevc), "IDR_W_RADL");
        assert!(!is_random_access(&pes_packet(0x100, &[0, 0, 1, 0x02, 0x01], false), Video::Hevc), "trailing picture");
        assert!(is_random_access(&pes_packet(0x100, &[0, 0, 1, 0xB3], false), Video::Mpeg2));
        assert!(!is_random_access(&continuation(0x100), Video::H264));
    }

    #[test]
    fn puts_sections_together_across_packets_and_checks_them() {
        // A PMT long enough to need two packets: many teletext entries, then the real streams.
        let page: &[u8] = &[0x56, 0x05, b'e', b'n', b'g', 0x09, 0x00];
        let mut streams: Vec<(u8, u16, &[u8])> = (0..18).map(|n| (0x06, 0x400 + n, page)).collect();
        streams.extend([(0x1B, 0x100, &[][..]), (0x0F, 0x101, &[][..])]);
        let pmt_packets = pmt(0x1000, &streams);
        assert_eq!(pmt_packets.len(), 2 * PACKET);
        let mut stream = pat(0x1000);
        stream.extend(&pmt_packets);
        let codecs = scan(&stream).codecs().unwrap();
        assert_eq!(codecs.video.map(|v| v.pid), Some(Some(0x100)));

        // A damaged PMT is ignored rather than misread.
        let mut damaged = pat(0x1000);
        let mut bad = pmt(0x1000, &[(0x1B, 0x100, &[])]);
        bad[20] ^= 0x01;
        damaged.extend(bad);
        damaged.extend(nulls(2));
        assert!(!scan(&damaged).knows_program());
    }

    #[test]
    fn copes_with_bytes_arriving_in_pieces_and_lost_sync() {
        let mut stream = pat(0x1000);
        stream.extend(nulls(2));
        stream.extend([0x00; 7]); // a broken packet's leftovers
        stream.extend(pmt(0x1000, &[(0x1B, 0x100, &[]), (0x81, 0x101, &[])]));
        stream.extend(pes_packet(0x100, &[], true));
        stream.extend(nulls(2));
        let mut scan = TsScan::default();
        for end in (1..=stream.len()).step_by(61) {
            scan.scan(&stream[..end]);
        }
        scan.scan(&stream);
        assert!(scan.is_ready());
        assert_eq!(scan.codecs().unwrap().audio.map(|a| a.codec), Some(Audio::Ac3));
    }

    #[test]
    fn not_transport_streams() {
        let html = b"<!DOCTYPE html><html><body>Maximum connections reached</body></html>".repeat(10);
        let scan = scan(&html);
        assert!(!scan.is_transport() && scan.codecs().is_none());
        assert_eq!(scan.start_bytes(&html), html, "handed over as it came");
    }

    fn flv(flags: u8, tags: &[(u8, &[u8])]) -> Vec<u8> {
        let mut bytes = vec![b'F', b'L', b'V', 1, flags, 0, 0, 0, 9, 0, 0, 0, 0];
        for &(kind, data) in tags {
            let size = data.len();
            bytes.extend([kind, (size >> 16) as u8, (size >> 8) as u8, size as u8, 0, 0, 0, 0, 0, 0, 0]);
            bytes.extend_from_slice(data);
            bytes.extend(((size + 11) as u32).to_be_bytes());
        }
        bytes
    }

    #[test]
    fn reads_flv_tags() {
        let script: &[u8] = &[2, 0, 10, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a'];
        let stream = flv(0x05, &[(18, script), (9, &[0x17, 0x00, 0, 0, 0]), (8, &[0xAF, 0x00, 0x12, 0x10])]);
        let scan = scan_flv(&stream).unwrap();
        assert!(scan.is_complete());
        assert_eq!(scan.codecs().video.map(|v| v.codec), Some(Video::H264));
        assert_eq!(scan.codecs().audio.map(|a| a.codec), Some(Audio::Aac));

        let enhanced = flv(0x05, &[(9, &[0x90, b'h', b'v', b'c', b'1']), (8, &[0x2F, 0x00])]);
        let scan = scan_flv(&enhanced).unwrap();
        assert_eq!(scan.codecs().video.map(|v| v.codec), Some(Video::Hevc));
        assert_eq!(scan.codecs().audio.map(|a| a.codec), Some(Audio::Mpeg));

        let partial = flv(0x05, &[(9, &[0x17, 0x00])]);
        let scan = scan_flv(&partial[..partial.len() - 5]).unwrap();
        assert!(!scan.is_complete(), "the tag isn't all there");
        assert_eq!(scan.codecs(), Codecs::unknown(), "announced streams are assumed and converted");
        assert!(scan_flv(b"\x47\x40\x00\x10").is_none());
    }

    #[test]
    fn reads_codecs_attributes() {
        let codecs = codecs_attribute("avc1.64001f,mp4a.40.2");
        assert_eq!(codecs.video.map(|v| v.codec), Some(Video::H264));
        assert_eq!(codecs.audio.map(|a| a.codec), Some(Audio::Aac));
        let codecs = codecs_attribute("hvc1.2.4.L150.B0, ec-3");
        assert_eq!((codecs.video.map(|v| v.codec), codecs.audio.map(|a| a.codec)), (Some(Video::Hevc), Some(Audio::Eac3)));
        assert_eq!(codecs_attribute("mp4a.40.34").audio.map(|a| a.codec), Some(Audio::Mpeg));
        assert_eq!(codecs_attribute("mp4a.40.5").video, None, "radio");
        assert!(codecs_attribute("stpp.ttml.im1t").is_empty());
    }

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn reads_fmp4_init_sections() {
        let track = |entry: &[u8; 4]| {
            let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
            stsd.extend(mp4_box(entry, &[0; 16]));
            mp4_box(b"trak", &mp4_box(b"mdia", &mp4_box(b"minf", &mp4_box(b"stbl", &mp4_box(b"stsd", &stsd)))))
        };
        let mut moov = mp4_box(b"mvhd", &[0; 20]);
        moov.extend(track(b"hvc1"));
        moov.extend(track(b"ac-3"));
        let mut init = mp4_box(b"ftyp", b"iso6mp41");
        init.extend(mp4_box(b"moov", &moov));
        let codecs = mp4_codecs(&init).unwrap();
        assert_eq!(codecs.video.map(|v| v.codec), Some(Video::Hevc));
        assert_eq!(codecs.audio.map(|a| a.codec), Some(Audio::Ac3));
        assert!(mp4_codecs(&mp4_box(b"ftyp", b"iso6")).is_none());
        assert!(mp4_codecs(&init[..30]).is_none(), "cut short");
    }

    #[test]
    fn reads_master_and_media_playlists() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",URI=\"audio/en.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,CODECS=\"avc1.4d401f,mp4a.40.2\",RESOLUTION=640x360\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4000000,AUDIO=\"aud\",CODECS=\"avc1.640028,mp4a.40.2\"\nhigh/index.m3u8\n";
        let found = variants(master);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1], Variant {
            uri: "high/index.m3u8".into(),
            bandwidth: 4_000_000,
            codecs: Some("avc1.640028,mp4a.40.2".into()),
            audio: Some("aud".into()),
        });
        assert!(has_separate_audio(master, "aud"));
        assert!(!has_separate_audio(master, "other"));
        let media = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg1.m4s\n#EXTINF:4,\nseg2.m4s\n";
        assert!(variants(media).is_empty());
        assert_eq!(media_parts(media), (Some("init.mp4".into()), Some("seg2.m4s".into())));
        assert_eq!(attribute_list("A=1,B=\"x, y\",C=z"), HashMap::from([("A", "1"), ("B", "x, y"), ("C", "z")]));
    }
}
