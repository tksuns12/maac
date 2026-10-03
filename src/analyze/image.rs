//! Spectrogram and piano-roll images for multimodal review, and a minimal
//! deterministic PNG writer (stored deflate blocks, no compression crate).

/// FFT length for spectrogram columns.
const FFT_SIZE: usize = 4096;
/// Spectrogram rows, log-spaced from `MIN_HZ` to `MAX_HZ` (or Nyquist).
pub(super) const ROWS: usize = 192;
pub(super) const MIN_HZ: f64 = 30.0;
pub(super) const MAX_HZ: f64 = 16_000.0;
/// Column intensities span this many dB below full scale.
const RANGE_DB: f64 = 96.0;
/// The widest image, in columns.
pub(super) const MAX_COLUMNS: u64 = 1200;

/// Frames per image column for a render of `frames` frames.
pub(super) fn frames_per_column(frames: u64) -> u64 {
    frames.div_ceil(MAX_COLUMNS).max(512)
}

pub(super) fn columns(frames: u64, hop: u64) -> u64 {
    frames.div_ceil(hop).max(1)
}

/// A streaming log-frequency magnitude spectrogram of a mono signal.
pub(super) struct Spectrogram {
    hop: u64,
    ring: Vec<f64>,
    position: usize,
    count: u64,
    window: Vec<f64>,
    window_sum: f64,
    /// For each row, top to bottom, the FFT bins it covers.
    rows: Vec<(usize, usize)>,
    pub max_hz: f64,
    /// One column of `ROWS` intensities (0–255) per hop.
    pub columns: Vec<Vec<u8>>,
}

impl Spectrogram {
    pub fn new(rate: u32, hop: u64) -> Self {
        let window: Vec<f64> = (0..FFT_SIZE)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / FFT_SIZE as f64).cos())
            .collect();
        let window_sum = window.iter().sum();
        let max_hz = MAX_HZ.min(f64::from(rate) / 2.0 * 0.98);
        let bin = |hz: f64| hz * FFT_SIZE as f64 / f64::from(rate);
        let edge = |r: f64| max_hz * (MIN_HZ / max_hz).powf(r / ROWS as f64);
        let rows = (0..ROWS)
            .map(|r| {
                // Row 0 is the highest band; `edge` falls as r grows.
                let high = bin(edge(r as f64));
                let low = bin(edge(r as f64 + 1.0));
                let first = low.round().max(1.0) as usize;
                let last = (high.round() as usize).max(first + 1).min(FFT_SIZE / 2);
                (first.min(last - 1), last)
            })
            .collect();
        Self {
            hop,
            ring: vec![0.0; FFT_SIZE],
            position: 0,
            count: 0,
            window,
            window_sum,
            rows,
            max_hz,
            columns: Vec::new(),
        }
    }

    pub fn push(&mut self, sample: f64) {
        self.ring[self.position] = sample;
        self.position = (self.position + 1) % FFT_SIZE;
        self.count += 1;
        if self.count.is_multiple_of(self.hop) {
            self.column();
        }
    }

    pub fn finish(mut self) -> Self {
        if !self.count.is_multiple_of(self.hop) || self.count == 0 {
            self.column();
        }
        self
    }

    fn column(&mut self) {
        let mut re: Vec<f64> = (0..FFT_SIZE)
            .map(|n| self.ring[(self.position + n) % FFT_SIZE] * self.window[n])
            .collect();
        let mut im = vec![0.0; FFT_SIZE];
        fft(&mut re, &mut im);
        let scale = 2.0 / self.window_sum;
        let column = self
            .rows
            .iter()
            .map(|&(first, last)| {
                let magnitude = (first..last)
                    .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale)
                    .fold(0.0, f64::max);
                let db = if magnitude > 0.0 {
                    20.0 * magnitude.log10()
                } else {
                    -RANGE_DB
                };
                (((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect();
        self.columns.push(column);
    }

    /// Render as RGB rows, top row first, with section boundaries marked.
    pub fn image(&self, width: usize, boundaries: &[usize]) -> Vec<u8> {
        let mut rgb = vec![0; width * ROWS * 3];
        for (x, column) in self.columns.iter().enumerate().take(width) {
            for (y, &value) in column.iter().enumerate() {
                let [r, g, b] = colormap(value);
                let i = (y * width + x) * 3;
                rgb[i..i + 3].copy_from_slice(&[r, g, b]);
            }
        }
        for &x in boundaries.iter().filter(|&&x| x < width) {
            for y in (0..ROWS).step_by(2) {
                let i = (y * width + x) * 3;
                rgb[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        rgb
    }
}

/// In-place iterative radix-2 complex FFT; the length is a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut length = 2;
    while length <= n {
        let angle = -2.0 * std::f64::consts::PI / length as f64;
        let (w_im, w_re) = angle.sin_cos();
        for start in (0..n).step_by(length) {
            let (mut c_re, mut c_im) = (1.0, 0.0);
            for k in 0..length / 2 {
                let (a, b) = (start + k, start + k + length / 2);
                let t_re = re[b] * c_re - im[b] * c_im;
                let t_im = re[b] * c_im + im[b] * c_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next = c_re * w_re - c_im * w_im;
                c_im = c_re * w_im + c_im * w_re;
                c_re = next;
            }
        }
        length <<= 1;
    }
}

/// Black through purple, red and yellow to near-white.
fn colormap(value: u8) -> [u8; 3] {
    const ANCHORS: [[f64; 3]; 6] = [
        [0.0, 0.0, 0.0],
        [40.0, 10.0, 90.0],
        [150.0, 30.0, 120.0],
        [230.0, 90.0, 50.0],
        [250.0, 200.0, 60.0],
        [255.0, 255.0, 230.0],
    ];
    let t = f64::from(value) / 255.0 * (ANCHORS.len() - 1) as f64;
    let i = (t.floor() as usize).min(ANCHORS.len() - 2);
    let f = t - i as f64;
    let mix = |c: usize| (ANCHORS[i][c] * (1.0 - f) + ANCHORS[i + 1][c] * f).round() as u8;
    [mix(0), mix(1), mix(2)]
}

/// One note or hit drawn on the piano roll.
pub(super) struct Mark {
    pub on_frame: u64,
    pub off_frame: u64,
    /// MIDI-style key for notes; `None` for hits, drawn in their own lanes.
    pub key: Option<i32>,
    /// Hit lane, for hits.
    pub lane: usize,
    pub color: usize,
}

const PALETTE: [[u8; 3]; 10] = [
    [86, 180, 233],
    [230, 159, 0],
    [0, 158, 115],
    [240, 228, 66],
    [204, 121, 167],
    [213, 94, 0],
    [0, 114, 178],
    [170, 170, 170],
    [120, 200, 120],
    [200, 120, 120],
];
/// The piano-roll colour of target `index`, as `#rrggbb`.
pub(super) fn color_hex(index: usize) -> String {
    let [r, g, b] = PALETTE[index % PALETTE.len()];
    format!("#{r:02x}{g:02x}{b:02x}")
}

const KEY_PIXELS: usize = 4;
const LANE_PIXELS: usize = 6;

/// Piano-roll RGB rows, top row first, and its height and key range.
pub(super) fn piano_roll(
    marks: &[Mark],
    lanes: usize,
    width: usize,
    frames_per_pixel: u64,
    boundaries: &[usize],
) -> (Vec<u8>, usize, Option<(i32, i32)>) {
    let keys = marks.iter().filter_map(|mark| mark.key);
    let range = keys
        .clone()
        .min()
        .zip(keys.max())
        .map(|(low, high)| (low - 1, high + 1));
    let note_rows = range.map_or(0, |(low, high)| (high - low + 1) as usize * KEY_PIXELS);
    let height = (note_rows + lanes * LANE_PIXELS).max(16);
    let mut rgb = vec![24; width * height * 3];
    let mut fill = |x0: usize, x1: usize, y0: usize, y1: usize, color: [u8; 3]| {
        for y in y0..y1.min(height) {
            for x in x0..x1.min(width) {
                let i = (y * width + x) * 3;
                rgb[i..i + 3].copy_from_slice(&color);
            }
        }
    };
    if let Some((low, high)) = range {
        // Faint lines at every C.
        for key in low..=high {
            if key.rem_euclid(12) == 0 {
                let y = (high - key) as usize * KEY_PIXELS + KEY_PIXELS - 1;
                fill(0, width, y, y + 1, [48, 48, 48]);
            }
        }
    }
    for &x in boundaries {
        fill(x, x + 1, 0, height, [110, 110, 110]);
    }
    for mark in marks {
        let x0 = (mark.on_frame / frames_per_pixel) as usize;
        let x1 = ((mark.off_frame / frames_per_pixel) as usize).max(x0 + 1);
        let (y0, y1) = match (mark.key, range) {
            (Some(key), Some((_, high))) => {
                let y = (high - key) as usize * KEY_PIXELS;
                (y, y + KEY_PIXELS - 1)
            }
            _ => {
                let y = note_rows + mark.lane * LANE_PIXELS;
                (y + 1, y + LANE_PIXELS - 1)
            }
        };
        fill(x0, x1, y0, y1, PALETTE[mark.color % PALETTE.len()]);
    }
    (rgb, height, range)
}

/// Encode RGB8 rows (top first) as a PNG with stored deflate blocks.
pub(super) fn png(width: usize, height: usize, rgb: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    for row in rgb.chunks(width * 3).take(height) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlace
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_published_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn fft_finds_a_bin_centred_sine() {
        let n = 64;
        let mut re: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * 5.0 * i as f64 / n as f64).sin())
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let magnitudes: Vec<f64> = (0..n / 2).map(|k| re[k].hypot(im[k])).collect();
        let peak = (0..n / 2)
            .max_by(|&a, &b| magnitudes[a].total_cmp(&magnitudes[b]))
            .unwrap();
        assert_eq!(peak, 5);
        assert!((magnitudes[5] - n as f64 / 2.0).abs() < 1e-9);
    }

    #[test]
    fn png_has_the_signature_and_dimensions() {
        let bytes = png(3, 2, &[0; 18]);
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        assert_eq!(&bytes[16..24], &[0, 0, 0, 3, 0, 0, 0, 2]);
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
    }
}
