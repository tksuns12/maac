//! Optional host-supplied audio processor hook for companion DAWs.
//!
//! MaaC core remains pure Rust with zero external plugin or OS dependencies.
//! When no [`HostProcessor`] is attached (the default for CLI, CI, offline
//! rendering, and checkpoints), [`DspEngine`](super::DspEngine) runs its
//! untouched bit-identical code path.

/// A sample-accurate note event within a rendered host block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostNoteEvent {
    /// Sample offset within the current block (`0..ctx.frames`).
    pub frame_offset: usize,
    /// `true` for Note On, `false` for Note Off.
    pub is_note_on: bool,
    /// Nearest 12-TET MIDI note number (`0..=127`).
    pub midi_note: u8,
    /// Normalized velocity in `0.0..=1.0`.
    pub velocity: f32,
    /// Exact fundamental frequency in Hertz (for microtonal / CLAP tuning).
    pub pitch_hz: f64,
}

/// Timing metadata for one host-processed audio block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostBlockContext {
    /// Song frame at sample offset 0 of this block.
    pub start_frame: u64,
    /// Number of frames in this block.
    pub frames: usize,
    /// Engine sample rate in Hz (e.g. `48_000.0`).
    pub sample_rate_hz: f64,
    /// Active tempo in BPM at `start_frame`.
    pub tempo_bpm: f64,
}

/// Plugin-agnostic host hook for companion DAWs (e.g. MaaC Studio hosting
/// CLAP or macOS Audio Unit instruments and effects).
///
/// Implementations are called from [`Playback`](super::Playback)'s producer
/// thread (never from the real-time audio callback [`Playback::read`](super::Playback::read)),
/// once per chunk (`chunk_frames`, typically 512 frames).
pub trait HostProcessor: Send {
    /// Fast-path check: return `false` when no external instrument overrides,
    /// node inserts, or master inserts are active.
    fn has_any_active(&self) -> bool;

    /// Whether `node_id`'s built-in synthesizer should be replaced by
    /// [`Self::process_instrument`].
    fn has_instrument_override(&self, node_id: &str) -> bool;

    /// Whether `node_id`'s output should pass through
    /// [`Self::process_node_insert`] before downstream MaaC nodes read it.
    fn has_node_insert(&self, node_id: &str) -> bool;

    /// Whether the final rendered output should pass through
    /// [`Self::process_master_insert`].
    fn has_master_insert(&self) -> bool;

    /// Render one block for an overridden instrument node into `out`
    /// (interleaved `ctx.frames * channels`, initialized to silence).
    fn process_instrument(
        &mut self,
        node_id: &str,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    );

    /// Process one block in place for a post-node insert effect chain
    /// (interleaved `ctx.frames * channels`).
    fn process_node_insert(
        &mut self,
        node_id: &str,
        ctx: HostBlockContext,
        channels: usize,
        io: &mut [f64],
    );

    /// Process one block in place for a post-master insert effect chain
    /// (interleaved `ctx.frames * channels`).
    fn process_master_insert(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]);

    /// Clear hanging notes and effect tails on seek, loop wrap, or engine reset.
    fn reset(&mut self, sample_rate_hz: f64, max_block_frames: usize);
}

/// Map a MaaC drum kit hit key to a conventional General MIDI drum note number
/// so drum tracks can also drive external drum instruments.
pub(crate) fn hit_key_to_midi(key: &str) -> u8 {
    let lower = key.to_ascii_lowercase();
    if lower.contains("kick") || lower == "bd" {
        36
    } else if lower.contains("rim") || lower.contains("sidestick") {
        37
    } else if lower.contains("clap") {
        39
    } else if lower.contains("snare") || lower == "sd" {
        38
    } else if lower.contains("open") && (lower.contains("hat") || lower.contains("hh")) {
        46
    } else if lower.contains("pedal") && (lower.contains("hat") || lower.contains("hh")) {
        44
    } else if lower.contains("hat") || lower.contains("hh") {
        42
    } else if lower.contains("crash") {
        49
    } else if lower.contains("ride") {
        51
    } else if lower.contains("low") && lower.contains("tom") {
        45
    } else if lower.contains("high") && lower.contains("tom") {
        50
    } else if lower.contains("tom") {
        47
    } else if lower.contains("cowbell") {
        56
    } else if lower.contains("tamb") {
        54
    } else if lower.contains("shaker") {
        70
    } else {
        36
    }
}
