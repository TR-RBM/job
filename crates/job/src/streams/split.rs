use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::quota::Quota;
use super::reader::{Batch, Reader, regular};
use super::{
    CHUNK, Chain, HEADER, Metadata, Mode, Part, Record, SEGMENT_BYTES, SOURCES, Segment, Source,
    Writer, invalid, message, segment_name,
};

const SEGMENT_LIMIT: usize = 16_384;

struct Open {
    file: File,
    number: u64,
    part: Part,
}

pub(super) struct Chains {
    quota: Quota,
    next_number: u64,
    open: [Option<Open>; SOURCES],
    head_used: [u64; SOURCES],
    head_closed: [bool; SOURCES],
    records: [u64; SOURCES],
}

impl Chains {
    pub(super) fn new(quota: Quota) -> Self {
        Self {
            quota,
            next_number: 0,
            open: std::array::from_fn(|_| None),
            head_used: [0; SOURCES],
            head_closed: [false; SOURCES],
            records: [0; SOURCES],
        }
    }
}

fn of(segment: &Segment, stream: Source, part: Part) -> bool {
    segment
        .chain
        .as_ref()
        .is_some_and(|chain| chain.stream == stream && chain.part == part)
}

impl Writer {
    pub(super) fn write_split(
        &mut self,
        source: Source,
        bytes: &[u8],
        timestamp: Option<u64>,
    ) -> io::Result<()> {
        let index = source.code() as usize - 1;
        let mut rest = bytes;
        while !rest.is_empty() {
            let chains = self.split.as_mut().ok_or_else(invalid)?;
            let (part, room) = if chains.head_closed[index] {
                (Part::Tail, CHUNK)
            } else {
                let room = chains.quota.head_bytes - chains.head_used[index];
                if room <= HEADER as u64 {
                    chains.head_closed[index] = true;
                    continue;
                }
                (Part::Head, ((room - HEADER as u64) as usize).min(CHUNK))
            };
            let (now, later) = rest.split_at(rest.len().min(room));
            rest = later;
            let at_ms = self.stamp(timestamp);
            let length = (HEADER + now.len()) as u64;
            let chains = self.split.as_ref().ok_or_else(invalid)?;
            let fits = chains.open[index].as_ref().is_some_and(|open| {
                open.part == part
                    && self
                        .meta
                        .segments
                        .iter()
                        .rev()
                        .find(|segment| segment.number == open.number)
                        .is_some_and(|segment| segment.bytes + length <= SEGMENT_BYTES)
            });
            if !fits {
                self.flush_split(index)?;
                self.open_segment(source, part)?;
            }
            Record::frame(
                &mut self.pending,
                self.meta.next_sequence,
                at_ms,
                source,
                now,
            )?;
            let chains = self.split.as_mut().ok_or_else(invalid)?;
            let number = chains.open[index].as_ref().ok_or_else(invalid)?.number;
            if part == Part::Head {
                chains.head_used[index] += length;
            }
            chains.records[index] += 1;
            let segment = self
                .meta
                .segments
                .iter_mut()
                .rev()
                .find(|segment| segment.number == number)
                .ok_or_else(invalid)?;
            segment.bytes += length;
            segment.records += 1;
            self.tally.written[index] += now.len() as u64;
            self.meta.next_sequence = self.meta.next_sequence.checked_add(1).ok_or_else(invalid)?;
        }
        self.flush_split(index)
    }

    fn flush_split(&mut self, index: usize) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let written = match self
            .split
            .as_mut()
            .and_then(|chains| chains.open[index].as_mut())
        {
            Some(open) => open.file.write_all(&self.pending),
            None => Err(invalid()),
        };
        self.pending.clear();
        written
    }

    fn open_segment(&mut self, source: Source, part: Part) -> io::Result<()> {
        let index = source.code() as usize - 1;
        let chains = self.split.as_mut().ok_or_else(invalid)?;
        chains.open[index] = None;
        let number = chains.next_number;
        chains.next_number = number.checked_add(1).ok_or_else(invalid)?;
        let tail_bytes = chains.quota.tail_bytes;
        let record = chains.records[index];
        if self.meta.segments.len() >= SEGMENT_LIMIT {
            return Err(invalid());
        }
        let file = Self::create_segment(&self.root, number)?;
        self.meta.segments.push(Segment {
            number,
            first_sequence: self.meta.next_sequence,
            bytes: 0,
            records: 0,
            chain: Some(Chain {
                stream: source,
                part,
                offset: self.tally.written[index],
                record,
            }),
        });
        let mut retired = Vec::new();
        if part == Part::Tail {
            let mut kept: u64 = self
                .meta
                .segments
                .iter()
                .filter(|segment| of(segment, source, Part::Tail))
                .map(|segment| segment.bytes)
                .sum();
            while let Some(position) = self
                .meta
                .segments
                .iter()
                .position(|segment| of(segment, source, Part::Tail))
            {
                let oldest = &self.meta.segments[position];
                if oldest.number == number || kept - oldest.bytes < tail_bytes {
                    break;
                }
                kept -= oldest.bytes;
                let segment = self.meta.segments.remove(position);
                self.meta.retired_bytes = self.meta.retired_bytes.saturating_add(segment.bytes);
                self.meta.retired_records =
                    self.meta.retired_records.saturating_add(segment.records);
                self.tally.dropped[index] += segment.bytes - segment.records * HEADER as u64;
                retired.push(segment.number);
            }
        }
        self.split.as_mut().ok_or_else(invalid)?.open[index] = Some(Open { file, number, part });
        self.doomed.extend(retired);
        self.changed()
    }
}

pub(super) fn validate(meta: &Metadata, quota: Quota) -> io::Result<()> {
    if !quota.valid()
        || meta.segments.len() > SEGMENT_LIMIT
        || meta.segments.iter().any(|segment| {
            segment.chain.is_none()
                || segment.bytes > SEGMENT_BYTES
                || segment
                    .records
                    .checked_mul((HEADER + 1) as u64)
                    .is_none_or(|minimum| minimum > segment.bytes)
        })
        || meta
            .segments
            .windows(2)
            .any(|pair| pair[0].number >= pair[1].number)
        || meta.segments.windows(2).any(|pair| {
            pair[0]
                .first_sequence
                .checked_add(pair[0].records.min(1))
                .is_none_or(|next| next > pair[1].first_sequence)
        })
    {
        return Err(invalid());
    }
    let records = meta
        .segments
        .iter()
        .try_fold(meta.retired_records, |sum, segment| {
            sum.checked_add(segment.records)
        })
        .ok_or_else(invalid)?;
    if records != meta.next_sequence {
        return Err(invalid());
    }
    let mut used = BTreeMap::<(u8, Part), (u64, u64, u64)>::new();
    for segment in &meta.segments {
        let chain = segment.chain.as_ref().ok_or_else(invalid)?;
        let entry = used.entry((chain.stream.code(), chain.part)).or_default();
        if chain.offset < entry.1 || chain.record < entry.2 {
            return Err(invalid());
        }
        entry.0 = entry.0.checked_add(segment.bytes).ok_or_else(invalid)?;
        entry.1 = chain.offset;
        entry.2 = chain.record;
    }
    if used.iter().any(|((_, part), (bytes, _, _))| match part {
        Part::Head => *bytes > quota.head_bytes,
        Part::Tail => *bytes > quota.tail_bytes + 2 * SEGMENT_BYTES,
    }) {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    number: u64,
    offset: u64,
}

#[derive(Default)]
pub struct State {
    cursors: BTreeMap<(u8, Part), Cursor>,
    bytes: [u64; SOURCES],
    records: [u64; SOURCES],
    trim_reported: bool,
}

impl State {
    pub(super) fn reported(&self) -> bool {
        self.trim_reported
    }

    pub(super) fn report(&mut self) {
        self.trim_reported = true;
    }
}

struct Peek {
    record: Record,
    cursor: Cursor,
    first: Option<(u64, u64)>,
}

fn chains(meta: &Metadata) -> io::Result<BTreeMap<(u8, Part), Vec<usize>>> {
    let mut chains = BTreeMap::<(u8, Part), Vec<usize>>::new();
    for (index, segment) in meta.segments.iter().enumerate() {
        let chain = segment.chain.as_ref().ok_or_else(invalid)?;
        chains
            .entry((chain.stream.code(), chain.part))
            .or_default()
            .push(index);
    }
    Ok(chains)
}

fn peek(
    root: &Path,
    meta: &Metadata,
    members: &[usize],
    cursor: &mut Option<Cursor>,
    held: &mut Option<(u64, File)>,
) -> io::Result<Option<Peek>> {
    loop {
        let wanted = cursor.map_or(0, |cursor| cursor.number);
        let Some(position) = members
            .iter()
            .position(|index| meta.segments[*index].number >= wanted)
        else {
            return Ok(None);
        };
        let index = members[position];
        let segment = &meta.segments[index];
        let offset = match cursor {
            Some(cursor) if cursor.number == segment.number => cursor.offset,
            _ => 0,
        };
        *cursor = Some(Cursor {
            number: segment.number,
            offset,
        });
        let growing = meta.growing(index);
        if held
            .as_ref()
            .is_none_or(|(number, _)| *number != segment.number)
        {
            *held = match regular(&root.join(segment_name(segment.number))) {
                Err(e) if e.kind() == io::ErrorKind::NotFound && !meta.complete => {
                    return Ok(None);
                }
                result => Some((segment.number, result?)),
            };
        }
        let file = &mut held.as_mut().ok_or_else(invalid)?.1;
        let size = file.metadata()?.len();
        if size > SEGMENT_BYTES || offset > size || (!growing && size != segment.bytes) {
            return Err(invalid());
        }
        if size == offset {
            match members.get(position + 1) {
                Some(next) => {
                    *cursor = Some(Cursor {
                        number: meta.segments[*next].number,
                        offset: 0,
                    });
                    continue;
                }
                None => return Ok(None),
            }
        }
        if size - offset < HEADER as u64 {
            return if growing { Ok(None) } else { Err(invalid()) };
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; HEADER];
        file.read_exact(&mut header)?;
        let length = Record::length(&header)?;
        if size - offset - (HEADER as u64) < length as u64 {
            return if growing { Ok(None) } else { Err(invalid()) };
        }
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        let record = Record::decode(&header, bytes)?;
        let chain = segment.chain.as_ref().ok_or_else(invalid)?;
        if record.source != chain.stream
            || (offset == 0 && record.sequence != segment.first_sequence)
        {
            return Err(invalid());
        }
        return Ok(Some(Peek {
            record,
            cursor: Cursor {
                number: segment.number,
                offset: offset + (HEADER + length) as u64,
            },
            first: (offset == 0).then_some((chain.offset, chain.record)),
        }));
    }
}

pub(super) fn snapshot(root: &Path, meta: &Metadata) -> io::Result<u64> {
    if meta.trimmed_bytes.is_some() {
        return Ok(0);
    }
    let mut stop = 0;
    for members in chains(meta)?.values() {
        let index = *members.last().ok_or_else(invalid)?;
        let segment = &meta.segments[index];
        let file = match regular(&root.join(segment_name(segment.number))) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && !meta.complete => continue,
            result => result?,
        };
        let size = file.metadata()?.len();
        if size > SEGMENT_BYTES {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(size).read_to_end(&mut bytes)?;
        let mut offset = 0;
        let mut next = segment.first_sequence;
        while bytes.len() - offset >= HEADER {
            let header = bytes[offset..offset + HEADER].try_into().unwrap();
            let length = Record::length(header)?;
            if bytes.len() - offset - HEADER < length {
                break;
            }
            let record = Record::decode(
                header,
                bytes[offset + HEADER..offset + HEADER + length].to_vec(),
            )?;
            if record.sequence < next {
                return Err(invalid());
            }
            next = record.sequence.checked_add(1).ok_or_else(invalid)?;
            offset += HEADER + length;
        }
        if meta.complete && offset != bytes.len() {
            return Err(invalid());
        }
        stop = stop.max(next);
    }
    if meta.complete && stop != meta.next_sequence {
        return Err(invalid());
    }
    Ok(stop)
}

fn stale(root: &Path, meta: &Metadata) -> io::Result<bool> {
    let newest = meta.segments.iter().map(|segment| segment.number).max();
    let hidden = newest.map_or(0, |number| number.saturating_add(1));
    if std::fs::symlink_metadata(root.join(segment_name(hidden))).is_ok() {
        return Ok(true);
    }
    let again = super::read_metadata(&root.join("streams.json"))?;
    Ok(again.complete
        || again.trimmed_bytes.is_some()
        || again.next_sequence != meta.next_sequence
        || again.segments.iter().map(|segment| segment.number).max() != newest)
}

pub(super) fn read(
    reader: &mut Reader,
    root: &Path,
    mut meta: Metadata,
    mut batch: Batch,
) -> io::Result<Batch> {
    batch.quota = meta.quota;
    if let Some(bytes) = meta.trimmed_bytes {
        if !reader.split.trim_reported {
            reader.split.trim_reported = true;
            batch.trimmed = Some(bytes);
        }
        batch.complete = true;
        return Ok(batch);
    }
    let mut chains = chains(&meta)?;
    let mut peeked = BTreeMap::<(u8, Part), Option<Peek>>::new();
    let mut held = BTreeMap::<(u8, Part), Option<(u64, File)>>::new();
    let mut confirmed = false;
    let mut consumed = 0;
    while consumed < 4 * CHUNK {
        if reader
            .stop_sequence
            .is_some_and(|end| reader.next_sequence >= end)
        {
            batch.complete = true;
            if meta.complete {
                batch.error = meta.error.clone().or(batch.error);
            } else if batch.job.state.terminal() {
                batch.error = Some(message("output recording is incomplete"));
            }
            return Ok(batch);
        }
        for (key, members) in &chains {
            if !peeked.contains_key(key) {
                let mut cursor = reader.split.cursors.get(key).copied();
                let found = peek(
                    root,
                    &meta,
                    members,
                    &mut cursor,
                    held.entry(*key).or_default(),
                )?;
                if let Some(cursor) = cursor {
                    reader.split.cursors.insert(*key, cursor);
                }
                peeked.insert(*key, found);
            }
        }
        let Some(key) = peeked
            .iter()
            .filter_map(|(key, peek)| peek.as_ref().map(|peek| (peek.record.sequence, *key)))
            .min()
            .map(|(_, key)| key)
        else {
            if meta.complete {
                if reader.next_sequence != meta.next_sequence {
                    return Err(invalid());
                }
                batch.complete = true;
                batch.error = meta.error.clone().or(batch.error);
            } else if batch.job.state.terminal() {
                batch.complete = true;
                batch.error = Some(message("output recording is incomplete"));
            }
            return Ok(batch);
        };
        let sequence = peeked[&key].as_ref().map_or(0, |peek| peek.record.sequence);
        if sequence < reader.next_sequence {
            return Err(invalid());
        }
        if sequence > reader.next_sequence {
            if !confirmed && !meta.complete {
                let fresh = super::read_metadata(&root.join("streams.json"))?;
                if fresh.job_id != meta.job_id
                    || fresh.attempt != meta.attempt
                    || fresh.mode != meta.mode
                    || fresh.quota != meta.quota
                {
                    return Err(invalid());
                }
                if fresh.trimmed_bytes.is_some() {
                    return Ok(batch);
                }
                meta = fresh;
                chains = self::chains(&meta)?;
                peeked.clear();
                confirmed = true;
                continue;
            }
            if !meta.complete && stale(root, &meta)? {
                return Ok(batch);
            }
            if sequence - reader.next_sequence > meta.retired_records {
                return if meta.complete {
                    Err(invalid())
                } else {
                    Ok(batch)
                };
            }
        }
        confirmed = false;
        let Some(Some(peek)) = peeked.remove(&key) else {
            return Err(invalid());
        };
        let record = peek.record;
        if reader
            .stop_sequence
            .is_some_and(|end| record.sequence >= end)
        {
            batch.complete = true;
            return Ok(batch);
        }
        if (meta.mode == Mode::Pty && matches!(record.source, Source::Stdout | Source::Stderr))
            || (meta.mode == Mode::Pipe && record.source == Source::Terminal)
            || matches!(record.source, Source::Combined | Source::End)
        {
            return Err(invalid());
        }
        let index = record.source.code() as usize - 1;
        if let Some((offset, number)) = peek.first {
            if offset < reader.split.bytes[index] || number < reader.split.records[index] {
                return Err(invalid());
            }
            if offset > reader.split.bytes[index] {
                let mut gap = Record::gap(
                    reader.next_sequence,
                    record.at_ms,
                    number - reader.split.records[index],
                    offset - reader.split.bytes[index],
                );
                gap.gap_stream = Some(record.source);
                batch.records.push(gap);
            }
            reader.split.bytes[index] = offset;
            reader.split.records[index] = number;
        }
        reader.split.bytes[index] += record.bytes.len() as u64;
        reader.split.records[index] += 1;
        reader.split.cursors.insert(key, peek.cursor);
        reader.next_sequence = record.sequence.checked_add(1).ok_or_else(invalid)?;
        consumed += record.bytes.len();
        batch.records.push(record);
    }
    Ok(batch)
}
