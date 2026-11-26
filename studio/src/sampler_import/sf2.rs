//! Pure-Rust SoundFont 2 (`.sf2`, RIFF `sfbk`) parser.

use std::collections::BTreeMap;
use std::path::Path;

use super::{sanitize_maac_ident, ImportedInstrument, ImportedSample, ImportedZone};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sf2PresetInfo {
    pub bank: u16,
    pub program: u16,
    pub name: String,
}

#[derive(Clone, Debug)]
struct Phdr {
    name: String,
    preset: u16,
    bank: u16,
    bag_ndx: u16,
}

#[derive(Clone, Debug)]
struct Inst {
    bag_ndx: u16,
}

#[derive(Clone, Copy, Debug)]
struct Bag {
    gen_ndx: u16,
}

#[derive(Clone, Copy, Debug)]
struct Gen {
    oper: u16,
    amount: u16,
}

#[derive(Clone, Debug)]
struct Shdr {
    name: String,
    start: u32,
    end: u32,
    start_loop: u32,
    end_loop: u32,
    sample_rate: u32,
    original_pitch: u8,
    sample_link: u16,
    sample_type: u16,
}

struct ParsedSf2 {
    smpl: Vec<i16>,
    phdrs: Vec<Phdr>,
    pbags: Vec<Bag>,
    pgens: Vec<Gen>,
    insts: Vec<Inst>,
    ibags: Vec<Bag>,
    igens: Vec<Gen>,
    shdrs: Vec<Shdr>,
}

fn read_u16(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

fn read_u32(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

fn read_ascii_20(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn parse_sf2_bytes(data: &[u8]) -> Result<ParsedSf2, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"sfbk" {
        return Err("not a valid RIFF SoundFont 2 (.sf2) file".into());
    }
    let mut smpl: Vec<i16> = Vec::new();
    let mut phdrs = Vec::new();
    let mut pbags = Vec::new();
    let mut pgens = Vec::new();
    let mut insts = Vec::new();
    let mut ibags = Vec::new();
    let mut igens = Vec::new();
    let mut shdrs = Vec::new();

    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = read_u32(data, pos + 4) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(data.len());
        if id == b"LIST" && body_end >= body_start + 4 {
            let list_type = &data[body_start..body_start + 4];
            let mut sub = body_start + 4;
            while sub + 8 <= body_end {
                let sid = &data[sub..sub + 4];
                let ssize = read_u32(data, sub + 4) as usize;
                let sstart = sub + 8;
                let send = sstart.saturating_add(ssize).min(body_end);
                let chunk = &data[sstart..send];
                match (list_type, sid) {
                    (b"sdta", b"smpl") => {
                        smpl.reserve(chunk.len() / 2);
                        for pair in chunk.as_chunks::<2>().0 {
                            smpl.push(i16::from_le_bytes([pair[0], pair[1]]));
                        }
                    }
                    (b"pdta", b"phdr") => {
                        for rec in chunk.as_chunks::<38>().0 {
                            phdrs.push(Phdr {
                                name: read_ascii_20(&rec[0..20]),
                                preset: read_u16(rec, 20),
                                bank: read_u16(rec, 22),
                                bag_ndx: read_u16(rec, 24),
                            });
                        }
                    }
                    (b"pdta", b"pbag") => {
                        for rec in chunk.as_chunks::<4>().0 {
                            pbags.push(Bag {
                                gen_ndx: read_u16(rec, 0),
                            });
                        }
                    }
                    (b"pdta", b"pgen") => {
                        for rec in chunk.as_chunks::<4>().0 {
                            pgens.push(Gen {
                                oper: read_u16(rec, 0),
                                amount: read_u16(rec, 2),
                            });
                        }
                    }
                    (b"pdta", b"inst") => {
                        for rec in chunk.as_chunks::<22>().0 {
                            insts.push(Inst {
                                bag_ndx: read_u16(rec, 20),
                            });
                        }
                    }
                    (b"pdta", b"ibag") => {
                        for rec in chunk.as_chunks::<4>().0 {
                            ibags.push(Bag {
                                gen_ndx: read_u16(rec, 0),
                            });
                        }
                    }
                    (b"pdta", b"igen") => {
                        for rec in chunk.as_chunks::<4>().0 {
                            igens.push(Gen {
                                oper: read_u16(rec, 0),
                                amount: read_u16(rec, 2),
                            });
                        }
                    }
                    (b"pdta", b"shdr") => {
                        for rec in chunk.as_chunks::<46>().0 {
                            shdrs.push(Shdr {
                                name: read_ascii_20(&rec[0..20]),
                                start: read_u32(rec, 20),
                                end: read_u32(rec, 24),
                                start_loop: read_u32(rec, 28),
                                end_loop: read_u32(rec, 32),
                                sample_rate: read_u32(rec, 36),
                                original_pitch: rec[40],
                                sample_link: read_u16(rec, 42),
                                sample_type: read_u16(rec, 44),
                            });
                        }
                    }
                    _ => {}
                }
                sub = send + (ssize & 1);
            }
        }
        pos = body_end + (size & 1);
    }

    if phdrs.len() < 2 || insts.len() < 2 || shdrs.len() < 2 || smpl.is_empty() {
        return Err(
            "SoundFont 2 file is missing required preset, instrument, or sample data".into(),
        );
    }
    Ok(ParsedSf2 {
        smpl,
        phdrs,
        pbags,
        pgens,
        insts,
        ibags,
        igens,
        shdrs,
    })
}

fn timecents_to_seconds(tc: i16) -> f32 {
    2.0_f32.powf(f32::from(tc) / 1200.0).clamp(0.001, 20.0)
}

/// List all `(bank, program, name)` presets stored in a `.sf2` file.
pub fn list_sf2_presets(path: &Path) -> Result<Vec<Sf2PresetInfo>, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let sf2 = parse_sf2_bytes(&bytes)?;
    let mut presets: Vec<Sf2PresetInfo> = sf2
        .phdrs
        .iter()
        .take(sf2.phdrs.len().saturating_sub(1))
        .map(|p| Sf2PresetInfo {
            bank: p.bank,
            program: p.preset,
            name: if p.name.is_empty() {
                format!("Bank {} Prog {}", p.bank, p.preset)
            } else {
                p.name.clone()
            },
        })
        .collect();
    presets.sort_by_key(|p| (p.bank, p.program));
    Ok(presets)
}

/// Parse one `(bank, program)` preset from a `.sf2` file into our unified
/// `ImportedInstrument` representation.
pub fn parse_sf2_preset(
    path: &Path,
    bank: u16,
    program: u16,
) -> Result<ImportedInstrument, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let sf2 = parse_sf2_bytes(&bytes)?;

    let preset_idx = sf2
        .phdrs
        .iter()
        .take(sf2.phdrs.len().saturating_sub(1))
        .position(|p| p.bank == bank && p.preset == program)
        .ok_or_else(|| format!("preset bank={bank} program={program} not found in SF2"))?;

    let phdr = &sf2.phdrs[preset_idx];
    let next_phdr = &sf2.phdrs[preset_idx + 1];
    let pbag_start = usize::from(phdr.bag_ndx);
    let pbag_end = usize::from(next_phdr.bag_ndx).min(sf2.pbags.len().saturating_sub(1));

    let mut samples_out: Vec<ImportedSample> = Vec::new();
    let mut sample_map: BTreeMap<(usize, Option<usize>, u8, bool), usize> = BTreeMap::new();
    let mut zones_out: Vec<ImportedZone> = Vec::new();
    let mut attack_s = 0.005_f32;
    let mut decay_s = 0.25_f32;
    let mut sustain = 0.85_f32;
    let mut release_s = 0.25_f32;
    let mut has_stereo = false;

    for pbag_idx in pbag_start..pbag_end {
        let pgen_start = usize::from(sf2.pbags[pbag_idx].gen_ndx);
        let pgen_end = usize::from(sf2.pbags[pbag_idx + 1].gen_ndx).min(sf2.pgens.len());
        let mut p_key_lo = 0u8;
        let mut p_key_hi = 127u8;
        let mut p_vel_lo = 0u8;
        let mut p_vel_hi = 127u8;
        let mut inst_id: Option<usize> = None;
        for g in &sf2.pgens[pgen_start..pgen_end] {
            match g.oper {
                43 => {
                    p_key_lo = (g.amount & 0xFF) as u8;
                    p_key_hi = (g.amount >> 8) as u8;
                }
                44 => {
                    p_vel_lo = (g.amount & 0xFF) as u8;
                    p_vel_hi = (g.amount >> 8) as u8;
                }
                41 => inst_id = Some(usize::from(g.amount)),
                _ => {}
            }
        }
        let Some(inst_idx) = inst_id else {
            continue;
        };
        if inst_idx + 1 >= sf2.insts.len() {
            continue;
        }
        let ibag_start = usize::from(sf2.insts[inst_idx].bag_ndx);
        let ibag_end =
            usize::from(sf2.insts[inst_idx + 1].bag_ndx).min(sf2.ibags.len().saturating_sub(1));

        let mut global_attack = None;
        let mut global_decay = None;
        let mut global_sustain = None;
        let mut global_release = None;

        for (offset, ibag_idx) in (ibag_start..ibag_end).enumerate() {
            let igen_start = usize::from(sf2.ibags[ibag_idx].gen_ndx);
            let igen_end = usize::from(sf2.ibags[ibag_idx + 1].gen_ndx).min(sf2.igens.len());
            let mut key_lo = p_key_lo;
            let mut key_hi = p_key_hi;
            let mut vel_lo = p_vel_lo;
            let mut vel_hi = p_vel_hi;
            let mut root_override: Option<u8> = None;
            let mut sample_modes: u16 = 0;
            let mut shdr_id: Option<usize> = None;

            for g in &sf2.igens[igen_start..igen_end] {
                match g.oper {
                    34 => {
                        let s = timecents_to_seconds(g.amount as i16);
                        if offset == 0 {
                            global_attack = Some(s);
                        }
                        attack_s = s;
                    }
                    36 => {
                        let s = timecents_to_seconds(g.amount as i16);
                        if offset == 0 {
                            global_decay = Some(s);
                        }
                        decay_s = s;
                    }
                    37 => {
                        let cb = (g.amount as i16).clamp(0, 1000) as f32;
                        let lvl = 10.0_f32.powf(-cb / 200.0).clamp(0.0, 1.0);
                        if offset == 0 {
                            global_sustain = Some(lvl);
                        }
                        sustain = lvl;
                    }
                    38 => {
                        let s = timecents_to_seconds(g.amount as i16);
                        if offset == 0 {
                            global_release = Some(s);
                        }
                        release_s = s;
                    }
                    43 => {
                        key_lo = key_lo.max((g.amount & 0xFF) as u8);
                        key_hi = key_hi.min((g.amount >> 8) as u8);
                    }
                    44 => {
                        vel_lo = vel_lo.max((g.amount & 0xFF) as u8);
                        vel_hi = vel_hi.min((g.amount >> 8) as u8);
                    }
                    53 => shdr_id = Some(usize::from(g.amount)),
                    54 => sample_modes = g.amount,
                    58 if g.amount <= 127 => {
                        root_override = Some(g.amount as u8);
                    }
                    _ => {}
                }
            }
            if let Some(a) = global_attack {
                attack_s = a;
            }
            if let Some(d) = global_decay {
                decay_s = d;
            }
            if let Some(s) = global_sustain {
                sustain = s;
            }
            if let Some(r) = global_release {
                release_s = r;
            }
            let Some(s_idx) = shdr_id else {
                continue;
            };
            if s_idx + 1 >= sf2.shdrs.len() || key_lo > key_hi || vel_lo > vel_hi {
                continue;
            }
            let sh = &sf2.shdrs[s_idx];
            // Skip right-channel companion of a linked stereo pair (already merged with left).
            if (sh.sample_type & 2) != 0 {
                continue;
            }
            let right_idx =
                if (sh.sample_type & 4) != 0 && usize::from(sh.sample_link) + 1 < sf2.shdrs.len() {
                    has_stereo = true;
                    Some(usize::from(sh.sample_link))
                } else {
                    None
                };
            let root = root_override.unwrap_or(if sh.original_pitch <= 127 {
                sh.original_pitch
            } else {
                60
            });
            let looped = (sample_modes & 1) != 0;
            let cache_key = (s_idx, right_idx, root, looped);
            let sample_index = if let Some(&existing) = sample_map.get(&cache_key) {
                existing
            } else {
                let start = (sh.start as usize).min(sf2.smpl.len());
                let end = (sh.end as usize).min(sf2.smpl.len());
                if end <= start {
                    continue;
                }
                let frames = end - start;
                let (channels, pcm) = if let Some(r_idx) = right_idx {
                    let rsh = &sf2.shdrs[r_idx];
                    let rstart = (rsh.start as usize).min(sf2.smpl.len());
                    let rend = (rsh.end as usize).min(sf2.smpl.len());
                    let rframes = rend.saturating_sub(rstart);
                    let len = frames.min(rframes);
                    let mut interleaved = Vec::with_capacity(len * 2);
                    for i in 0..len {
                        interleaved.push(f32::from(sf2.smpl[start + i]) / 32768.0);
                        interleaved.push(f32::from(sf2.smpl[rstart + i]) / 32768.0);
                    }
                    (2u8, interleaved)
                } else {
                    let mono: Vec<f32> = sf2.smpl[start..end]
                        .iter()
                        .map(|&v| f32::from(v) / 32768.0)
                        .collect();
                    (1u8, mono)
                };
                let loop_range =
                    if looped && sh.end_loop > sh.start_loop && sh.start_loop >= sh.start {
                        let ls = sh.start_loop - sh.start;
                        let le = (sh.end_loop - sh.start)
                            .min((pcm.len() / usize::from(channels)) as u32);
                        (le > ls + 1).then_some((ls, le))
                    } else {
                        None
                    };
                let idx = samples_out.len();
                samples_out.push(ImportedSample {
                    id: format!("s{}_{}", idx + 1, sanitize_maac_ident(&sh.name)),
                    rate_hz: sh.sample_rate.max(8_000),
                    channels,
                    root_midi: root,
                    loop_range,
                    samples: pcm,
                });
                sample_map.insert(cache_key, idx);
                idx
            };
            zones_out.push(ImportedZone {
                sample_index,
                low_key: key_lo,
                high_key: key_hi,
                low_vel: vel_lo,
                high_vel: vel_hi,
            });
        }
    }

    if zones_out.is_empty() || samples_out.is_empty() {
        return Err(format!(
            "SF2 preset '{}' contained no playable zones",
            phdr.name
        ));
    }

    let name = if phdr.name.is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sf2_instrument".into())
    } else {
        phdr.name.clone()
    };

    Ok(ImportedInstrument {
        slug: sanitize_maac_ident(&name),
        name,
        channels: if has_stereo { 2 } else { 1 },
        attack_s,
        decay_s,
        sustain,
        release_s,
        samples: samples_out,
        zones: zones_out,
        warnings: Vec::new(),
    })
}
