//! Immutable decoded PCM and bounded, frame-oriented source views.

use crate::{audio_asset::AudioAsset, plan::PlanError};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};

const CACHE_PAGE_COUNT: usize = 2;
const TARGET_PAGE_PAYLOAD_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub(crate) struct AudioBuffer {
    rate_hz: u32,
    channels: u8,
    frames: u64,
    byte_len: usize,
    external_disk: bool,
    storage: SampleStorage,
}

#[derive(Debug)]
enum SampleStorage {
    Memory(Arc<[f32]>),
    Disk(DiskStorage),
}

#[derive(Debug)]
struct DiskStorage {
    asset_id: String,
    frames: u64,
    frame_bytes: usize,
    page_frames: u64,
    page_payload_bytes: usize,
    state: Mutex<DiskState>,
}

#[derive(Debug)]
struct DiskState {
    file: File,
    pages: [CachePage; CACHE_PAGE_COUNT],
    next_victim: usize,
}

#[derive(Debug)]
struct CachePage {
    start_frame: Option<u64>,
    valid_bytes: usize,
    bytes: Box<[u8]>,
}

impl AudioBuffer {
    pub(crate) fn from_asset(asset: &AudioAsset) -> Result<Self, PlanError> {
        let samples = asset.decode()?;
        let byte_len = samples.len() * std::mem::size_of::<f32>();
        Ok(Self {
            rate_hz: asset.rate_hz,
            channels: asset.channels,
            frames: asset.frames,
            byte_len,
            external_disk: false,
            storage: SampleStorage::Memory(samples),
        })
    }

    pub(crate) fn from_asset_disk(asset: &AudioAsset) -> Result<Self, PlanError> {
        asset.validate()?;
        let storage = match &asset.disk {
            Some(snapshot) => DiskStorage::from_snapshot(asset, snapshot)?,
            None => DiskStorage::new(asset)?,
        };
        Ok(Self {
            rate_hz: asset.rate_hz,
            channels: asset.channels,
            frames: asset.frames,
            byte_len: asset
                .disk
                .as_ref()
                .map_or(asset.bytes.len(), |disk| disk.bytes as usize),
            external_disk: asset.disk.is_some(),
            storage: SampleStorage::Disk(storage),
        })
    }

    pub(crate) fn rate_hz(&self) -> u32 {
        self.rate_hz
    }
    pub(crate) fn channels(&self) -> u8 {
        self.channels
    }
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }
    pub(crate) fn byte_len(&self) -> usize {
        self.byte_len
    }
    pub(crate) fn is_external_disk(&self) -> bool {
        self.external_disk
    }

    pub(crate) fn slice(
        &self,
        start: u64,
        end: u64,
        reverse: bool,
    ) -> Result<AudioSlice<'_>, PlanError> {
        if start >= end || end > self.frames {
            return Err(PlanError {
                code: "E_RANGE".into(),
                path: "audio.source".into(),
                message: "source slice must satisfy 0 <= start < end <= asset frames".into(),
                span: None,
            });
        }
        Ok(AudioSlice {
            buffer: self,
            start,
            end,
            reverse,
        })
    }

    /// Own a validated view without copying or decoding the shared PCM again.
    pub(crate) fn owned_slice(
        self: &Arc<Self>,
        start: u64,
        end: u64,
        reverse: bool,
    ) -> Result<OwnedAudioSlice, PlanError> {
        self.slice(start, end, reverse)?;
        Ok(OwnedAudioSlice {
            buffer: Arc::clone(self),
            start,
            end,
            reverse,
        })
    }

    /// Whole-buffer sampling also supports the empty assets accepted by kits.
    pub(crate) fn interpolate(
        &self,
        index: u64,
        fraction: f64,
        channel: usize,
    ) -> Result<f64, PlanError> {
        AudioSlice {
            buffer: self,
            start: 0,
            end: self.frames,
            reverse: false,
        }
        .interpolate(index, fraction, channel)
    }

    fn sample(&self, frame: u64, channel: usize) -> Result<f64, PlanError> {
        debug_assert!(frame < self.frames);
        debug_assert!(channel < usize::from(self.channels));
        match &self.storage {
            SampleStorage::Memory(samples) => {
                let index = frame as usize * usize::from(self.channels) + channel;
                Ok(f64::from(samples[index]))
            }
            SampleStorage::Disk(storage) => storage.sample(frame, channel).map(f64::from),
        }
    }
}

impl DiskStorage {
    fn from_snapshot(
        asset: &AudioAsset,
        snapshot: &crate::disk_media::DiskAsset,
    ) -> Result<Self, PlanError> {
        let frame_bytes = usize::from(asset.channels) * std::mem::size_of::<f32>();
        let page_payload_bytes = (TARGET_PAGE_PAYLOAD_BYTES / frame_bytes) * frame_bytes;
        let page_frames = (page_payload_bytes / frame_bytes) as u64;
        let allocated_page_bytes = page_payload_bytes.min(snapshot.bytes as usize);
        let pages = [
            CachePage::new(allocated_page_bytes, &asset.id)?,
            CachePage::new(allocated_page_bytes, &asset.id)?,
        ];
        let file = snapshot.file.try_clone().map_err(|error| {
            disk_error(
                &asset.id,
                format!("cannot clone media snapshot handle: {error}"),
            )
        })?;
        Ok(Self {
            asset_id: asset.id.clone(),
            frames: asset.frames,
            frame_bytes,
            page_frames,
            page_payload_bytes: allocated_page_bytes,
            state: Mutex::new(DiskState {
                file,
                pages,
                next_victim: 0,
            }),
        })
    }

    fn new(asset: &AudioAsset) -> Result<Self, PlanError> {
        let frame_bytes = usize::from(asset.channels) * std::mem::size_of::<f32>();
        let page_payload_bytes = (TARGET_PAGE_PAYLOAD_BYTES / frame_bytes) * frame_bytes;
        let page_frames = (page_payload_bytes / frame_bytes) as u64;
        let allocated_page_bytes = page_payload_bytes.min(asset.bytes.len());
        let pages = [
            CachePage::new(allocated_page_bytes, &asset.id)?,
            CachePage::new(allocated_page_bytes, &asset.id)?,
        ];
        let mut file = tempfile::tempfile().map_err(|error| {
            disk_error(
                &asset.id,
                format!("cannot create private PCM snapshot: {error}"),
            )
        })?;
        file.write_all(&asset.bytes).map_err(|error| {
            disk_error(
                &asset.id,
                format!("cannot write exact PCM snapshot bytes: {error}"),
            )
        })?;
        file.flush().map_err(|error| {
            disk_error(
                &asset.id,
                format!("cannot flush exact PCM snapshot bytes: {error}"),
            )
        })?;
        file.seek(SeekFrom::Start(0)).map_err(|error| {
            disk_error(
                &asset.id,
                format!("cannot rewind private PCM snapshot: {error}"),
            )
        })?;
        Ok(Self {
            asset_id: asset.id.clone(),
            frames: asset.frames,
            frame_bytes,
            page_frames,
            page_payload_bytes: allocated_page_bytes,
            state: Mutex::new(DiskState {
                file,
                pages,
                next_victim: 0,
            }),
        })
    }

    fn sample(&self, frame: u64, channel: usize) -> Result<f32, PlanError> {
        debug_assert!(frame < self.frames);
        let page_start = (frame / self.page_frames) * self.page_frames;
        let mut state = self.state.lock().map_err(|_| {
            disk_error(
                &self.asset_id,
                "cannot lock the private PCM page cache after poisoning",
            )
        })?;
        let slot = match state
            .pages
            .iter()
            .position(|page| page.start_frame == Some(page_start))
        {
            Some(slot) => slot,
            None => {
                let slot = state
                    .pages
                    .iter()
                    .position(|page| page.start_frame.is_none())
                    .unwrap_or(state.next_victim);
                let frames_to_read = self.page_frames.min(self.frames - page_start);
                let bytes_to_read = frames_to_read as usize * self.frame_bytes;
                let byte_offset = page_start * self.frame_bytes as u64;
                let DiskState {
                    file,
                    pages,
                    next_victim,
                } = &mut *state;
                pages[slot].start_frame = None;
                pages[slot].valid_bytes = 0;
                #[cfg(unix)]
                let read_result = {
                    use std::os::unix::fs::FileExt;
                    file.read_exact_at(&mut pages[slot].bytes[..bytes_to_read], byte_offset)
                };
                #[cfg(not(unix))]
                let read_result = file.seek(SeekFrom::Start(byte_offset)).and_then(|_| {
                    std::io::Read::read_exact(file, &mut pages[slot].bytes[..bytes_to_read])
                });
                read_result.map_err(|error| {
                    disk_error(
                        &self.asset_id,
                        format!(
                            "cannot read PCM snapshot bytes {byte_offset}..{}: {error}",
                            byte_offset + bytes_to_read as u64
                        ),
                    )
                })?;
                pages[slot].start_frame = Some(page_start);
                pages[slot].valid_bytes = bytes_to_read;
                *next_victim = (slot + 1) % CACHE_PAGE_COUNT;
                slot
            }
        };
        let byte_in_page =
            (frame - page_start) as usize * self.frame_bytes + channel * std::mem::size_of::<f32>();
        let page = &state.pages[slot];
        debug_assert_eq!(page.bytes.len(), self.page_payload_bytes);
        debug_assert!(byte_in_page + 4 <= page.valid_bytes);
        Ok(f32::from_le_bytes(
            page.bytes[byte_in_page..byte_in_page + 4]
                .try_into()
                .unwrap(),
        ))
    }
}

impl CachePage {
    fn new(byte_len: usize, asset_id: &str) -> Result<Self, PlanError> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(byte_len).map_err(|_| PlanError {
            code: "E_RESOURCE_LIMIT".into(),
            path: format!("audio_assets.{asset_id}"),
            message: format!("cannot allocate {byte_len}-byte PCM cache page"),
            span: None,
        })?;
        bytes.resize(byte_len, 0);
        Ok(Self {
            start_frame: None,
            valid_bytes: 0,
            bytes: bytes.into_boxed_slice(),
        })
    }
}

fn disk_error(asset_id: &str, message: impl Into<String>) -> PlanError {
    PlanError {
        code: "E_IO".into(),
        path: format!("audio_assets.{asset_id}"),
        message: message.into(),
        span: None,
    }
}

#[derive(Debug)]
pub(crate) struct OwnedAudioSlice {
    buffer: Arc<AudioBuffer>,
    start: u64,
    end: u64,
    reverse: bool,
}
impl OwnedAudioSlice {
    pub(crate) fn channels(&self) -> u8 {
        self.buffer.channels
    }
    pub(crate) fn interpolate(
        &self,
        index: u64,
        fraction: f64,
        channel: usize,
    ) -> Result<f64, PlanError> {
        AudioSlice {
            buffer: &self.buffer,
            start: self.start,
            end: self.end,
            reverse: self.reverse,
        }
        .interpolate(index, fraction, channel)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AudioSlice<'a> {
    buffer: &'a AudioBuffer,
    start: u64,
    end: u64,
    reverse: bool,
}

impl AudioSlice<'_> {
    fn sample(&self, index: u64, channel: usize) -> Result<f64, PlanError> {
        if index >= self.end - self.start {
            return Ok(0.);
        }
        let frame = if self.reverse {
            self.end - 1 - index
        } else {
            self.start + index
        };
        self.buffer.sample(frame, channel)
    }

    /// The caller supplies a normalized fractional coordinate and valid channel.
    /// These are programmer invariants, not unchecked source-document input.
    pub(crate) fn interpolate(
        &self,
        index: u64,
        fraction: f64,
        channel: usize,
    ) -> Result<f64, PlanError> {
        assert!(fraction.is_finite() && (0.0..1.0).contains(&fraction));
        assert!(channel < self.buffer.channels as usize);
        if index >= self.end - self.start {
            return Ok(0.);
        }
        let left = self.sample(index, channel)?;
        let right = self.sample(index + 1, channel)?;
        Ok((1. - fraction) * left + fraction * right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        audio_asset::{AudioAsset, CORE_AUDIO_FORMAT},
        bundle::sha256_digest,
    };

    fn asset(channels: u8, values: &[f32]) -> AudioAsset {
        let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        AudioAsset {
            id: "sample".into(),
            format: CORE_AUDIO_FORMAT.into(),
            rate_hz: 24000,
            channels,
            frames: (values.len() / channels as usize) as u64,
            hash: sha256_digest(&bytes),
            bytes,
            disk: None,
        }
    }

    fn buffer(channels: u8, values: &[f32]) -> AudioBuffer {
        AudioBuffer::from_asset(&asset(channels, values)).unwrap()
    }

    fn disk_buffer(channels: u8, values: &[f32]) -> AudioBuffer {
        AudioBuffer::from_asset_disk(&asset(channels, values)).unwrap()
    }

    #[test]
    fn owned_slices_share_one_decoded_buffer_and_validate_once() {
        let buffer = Arc::new(buffer(1, &[1., 2., 4.]));
        let first = buffer.owned_slice(0, 3, false).unwrap();
        let second = buffer.owned_slice(1, 3, true).unwrap();
        assert!(Arc::ptr_eq(&first.buffer, &buffer));
        assert!(Arc::ptr_eq(&second.buffer, &buffer));
        assert_eq!(Arc::strong_count(&buffer), 3);
        assert_eq!(second.interpolate(0, 0.5, 0).unwrap(), 3.);
        assert!(buffer.owned_slice(1, 4, false).is_err());
    }
    #[test]
    fn mono_slice_interpolation_has_zero_neighbors_in_both_directions() {
        let b = buffer(1, &[99., 2., 6., 88.]);
        for (reverse, expected) in [(false, [2., 4., 6., 3.]), (true, [6., 4., 2., 1.])] {
            let slice = b.slice(1, 3, reverse).unwrap();
            for ((index, fraction), value) in [(0, 0.), (0, 0.5), (1, 0.), (1, 0.5)]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(slice.interpolate(index, fraction, 0).unwrap(), value);
            }
            assert_eq!(slice.interpolate(2, 0.5, 0).unwrap(), 0.);
            assert_eq!(slice.interpolate(u64::MAX, 0.5, 0).unwrap(), 0.);
        }
    }
    #[test]
    fn stereo_reverse_preserves_channel_order_and_slice_edges() {
        let b = buffer(2, &[99., 98., 2., 20., 6., 60., 88., 87.]);
        for (reverse, first, last) in [(false, [2., 20.], [6., 60.]), (true, [6., 60.], [2., 20.])]
        {
            let slice = b.slice(1, 3, reverse).unwrap();
            for channel in 0..2 {
                assert_eq!(slice.interpolate(0, 0., channel).unwrap(), first[channel]);
                assert_eq!(
                    slice.interpolate(0, 0.5, channel).unwrap(),
                    [4., 40.][channel]
                );
                assert_eq!(
                    slice.interpolate(1, 0.5, channel).unwrap(),
                    last[channel] * 0.5
                );
            }
        }
    }

    #[test]
    fn disk_stereo_sampling_preserves_forward_reverse_and_metadata() {
        let buffer = disk_buffer(2, &[1., 10., 3., 30., 7., 70.]);
        assert_eq!(
            (
                buffer.rate_hz(),
                buffer.channels(),
                buffer.frames(),
                buffer.byte_len()
            ),
            (24000, 2, 3, 24)
        );
        assert!(matches!(buffer.storage, SampleStorage::Disk(_)));
        for (reverse, first, middle) in
            [(false, [1., 10.], [2., 20.]), (true, [7., 70.], [5., 50.])]
        {
            let slice = buffer.slice(0, 3, reverse).unwrap();
            for channel in 0..2 {
                assert_eq!(slice.interpolate(0, 0., channel).unwrap(), first[channel]);
                assert_eq!(slice.interpolate(0, 0.5, channel).unwrap(), middle[channel]);
            }
        }
    }

    #[test]
    fn disk_interpolation_crosses_a_frame_aligned_page_boundary() {
        let page_frames = TARGET_PAGE_PAYLOAD_BYTES / std::mem::size_of::<f32>();
        let mut values = vec![0.; page_frames + 1];
        values[page_frames - 1] = 2.;
        values[page_frames] = 6.;
        let buffer = disk_buffer(1, &values);
        assert_eq!(
            buffer
                .interpolate((page_frames - 1) as u64, 0.25, 0)
                .unwrap(),
            (1. - 0.25) * 2. + 0.25 * 6.
        );
        let SampleStorage::Disk(storage) = &buffer.storage else {
            unreachable!()
        };
        let state = storage.state.lock().unwrap();
        assert_eq!(storage.page_frames, page_frames as u64);
        assert_eq!(
            state
                .pages
                .iter()
                .filter_map(|page| page.start_frame)
                .collect::<Vec<_>>(),
            [0, page_frames as u64]
        );
    }

    #[test]
    fn disk_slice_right_endpoint_is_zero_before_following_asset_frames() {
        let buffer = disk_buffer(1, &[99., 2., 6., 88.]);
        let slice = buffer.slice(1, 3, false).unwrap();
        assert_eq!(slice.interpolate(1, 0.5, 0).unwrap(), 3.);
        assert_eq!(slice.interpolate(2, 0.5, 0).unwrap(), 0.);
    }

    #[test]
    fn disk_cache_is_fixed_bounded_shared_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AudioBuffer>();

        let page_frames = TARGET_PAGE_PAYLOAD_BYTES / std::mem::size_of::<f32>();
        let buffer = Arc::new(disk_buffer(1, &vec![1.; page_frames * 3 + 1]));
        let first = buffer.owned_slice(0, buffer.frames(), false).unwrap();
        let second = buffer.owned_slice(0, buffer.frames(), true).unwrap();
        assert!(Arc::ptr_eq(&first.buffer, &second.buffer));
        for index in [0, page_frames, page_frames * 2] {
            assert_eq!(buffer.interpolate(index as u64, 0., 0).unwrap(), 1.);
        }
        let SampleStorage::Disk(storage) = &buffer.storage else {
            unreachable!()
        };
        let state = storage.state.lock().unwrap();
        assert_eq!(state.pages.len(), CACHE_PAGE_COUNT);
        assert_eq!(
            state
                .pages
                .iter()
                .map(|page| page.bytes.len())
                .sum::<usize>(),
            CACHE_PAGE_COUNT * TARGET_PAGE_PAYLOAD_BYTES
        );
        assert!(state.pages.iter().all(|page| page.start_frame.is_some()));
    }

    #[test]
    fn disk_read_failures_are_asset_qualified_errors_not_silence() {
        let buffer = disk_buffer(1, &[1., 2.]);
        let SampleStorage::Disk(storage) = &buffer.storage else {
            unreachable!()
        };
        storage.state.lock().unwrap().file.set_len(0).unwrap();
        let error = buffer.interpolate(0, 0., 0).unwrap_err();
        assert_eq!(error.code, "E_IO");
        assert_eq!(error.path, "audio_assets.sample");
        assert!(error.message.contains("cannot read PCM snapshot bytes"));
    }
    #[test]
    fn decoded_bits_and_metadata_are_preserved() {
        let bits = [
            0,
            0x80000000,
            1,
            0x80000001,
            2f32.to_bits(),
            (-3f32).to_bits(),
        ];
        let values: Vec<_> = bits.into_iter().map(f32::from_bits).collect();
        let b = buffer(2, &values);
        assert_eq!(
            (b.rate_hz(), b.channels(), b.frames(), b.byte_len()),
            (24000, 2, 3, 24)
        );
        let SampleStorage::Memory(samples) = &b.storage else {
            unreachable!()
        };
        assert_eq!(
            samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            bits
        );
    }
    #[test]
    fn rejects_empty_reversed_and_out_of_asset_slices() {
        let b = buffer(1, &[1., 2.]);
        for (a, z) in [(0, 0), (1, 1), (2, 1), (0, 3), (u64::MAX, u64::MAX)] {
            assert_eq!(b.slice(a, z, false).unwrap_err().code, "E_RANGE");
        }
        let empty = buffer(1, &[]);
        assert!(empty.slice(0, 1, false).is_err());
        assert_eq!(empty.interpolate(0, 0.5, 0).unwrap(), 0.);
        let empty_disk = disk_buffer(1, &[]);
        assert!(empty_disk.slice(0, 1, false).is_err());
        assert_eq!(empty_disk.interpolate(0, 0.5, 0).unwrap(), 0.);
    }
}
