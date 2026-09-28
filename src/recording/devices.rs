//! Bounded discovery and selection, shared by native capture and read-only listing.
use super::{recording_error, CliError, InputDevice, InputDeviceInfo, RecordingMetadata};
use std::collections::BTreeSet;

const ATTEMPTS: usize = 3;
pub(super) const MAX_PROPERTY_BYTES: usize = 256 * 1024;
const MAX_DEVICES: usize = 1024;
const MAX_CHANNELS: u32 = 4096;

pub(super) trait Provider {
    fn ids(&mut self) -> Result<Vec<u32>, CliError>;
    fn default_input(&mut self) -> Result<u32, CliError>;
    fn info(&mut self, id: u32) -> Result<Option<InputDeviceInfo>, CliError>;
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResolvedInput {
    pub object: u32,
    pub info: InputDeviceInfo,
}

fn inspect(provider: &mut impl Provider) -> Result<Vec<ResolvedInput>, CliError> {
    let ids = provider.ids()?;
    if ids.len() > MAX_DEVICES
        || ids.contains(&0)
        || ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(recording_error(
            "macOS returned an invalid or oversized device list",
        ));
    }
    let default = provider.default_input()?;
    let mut found = Vec::new();
    let mut uids = BTreeSet::new();
    for object in ids {
        if let Some(mut info) = provider.info(object)? {
            if info.uid.is_empty()
                || info.uid.len() > 4096
                || info.uid.contains('\0')
                || info.name.is_empty()
                || info.name.len() > 4096
                || info.input_channels == 0
                || info.input_channels > MAX_CHANNELS
                || info
                    .hardware_rate_hz
                    .is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
                || (info.available && info.hardware_rate_hz.is_none())
            {
                return Err(recording_error(
                    "macOS returned invalid input device metadata",
                ));
            }
            if !uids.insert(info.uid.clone()) {
                return Err(recording_error(
                    "macOS returned ambiguous input device UIDs",
                ));
            }
            info.is_default = object == default;
            found.push(ResolvedInput { object, info });
        }
    }
    found.sort_by(|a, b| a.info.uid.cmp(&b.info.uid));
    Ok(found)
}

pub(super) fn snapshot(provider: &mut impl Provider) -> Result<Vec<ResolvedInput>, CliError> {
    let mut failure = recording_error("input devices changed during discovery");
    for _ in 0..ATTEMPTS {
        match inspect(provider).and_then(|first| inspect(provider).map(|second| (first, second))) {
            Ok((first, second)) if first == second => return Ok(first),
            Ok(_) => failure = recording_error("input devices changed during discovery"),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

pub(super) fn resolve(
    provider: &mut impl Provider,
    requested: Option<&str>,
) -> Result<ResolvedInput, CliError> {
    let found = snapshot(provider)?;
    let selected = found
        .into_iter()
        .find(|device| match requested {
            Some(uid) => device.info.uid == uid,
            None => device.info.is_default,
        })
        .ok_or_else(|| {
            recording_error(match requested {
                Some(_) => "requested UID does not identify an input device",
                None => "macOS has no default input device",
            })
        })?;
    if !selected.info.available {
        return Err(recording_error("selected input device is unavailable"));
    }
    Ok(selected)
}

pub(super) fn revalidate(
    provider: &mut impl Provider,
    selected: &ResolvedInput,
) -> Result<(), CliError> {
    let actual = resolve(provider, Some(&selected.info.uid))?;
    if actual.object != selected.object
        || actual.info.hardware_rate_hz != selected.info.hardware_rate_hz
        || actual.info.input_channels != selected.info.input_channels
    {
        return Err(recording_error(
            "selected microphone disappeared or changed before capture",
        ));
    }
    Ok(())
}

pub(super) fn prepare_and_authorize(
    provider: &mut impl Provider,
    duration: u32,
    requested: Option<&str>,
    authorize: impl FnOnce() -> Result<(), CliError>,
) -> Result<(ResolvedInput, InputDevice), CliError> {
    let selected = resolve(provider, requested)?;
    let provenance = InputDevice {
        selection: if requested.is_some() {
            "explicit-uid"
        } else {
            "system-default"
        }
        .into(),
        uid: selected.info.uid.clone(),
        hardware_rate_hz: selected.info.hardware_rate_hz,
    };
    // JSON can expand control characters well beyond their UTF-8 byte count.
    // Fail this known metadata limit before requesting microphone permission.
    RecordingMetadata::for_capture(
        duration,
        provenance.clone(),
        format!("sha256:{}", "0".repeat(64)),
    )
    .bounded_bytes()?;
    authorize()?;
    revalidate(provider, &selected)?;
    Ok((selected, provenance))
}

pub(super) enum ReadFailure {
    Resized,
    Failed(CliError),
}

/// Storage has pointer alignment for AudioBufferList and never exceeds the cap.
/// A size/data race receives a finite fresh-size retry; invalid output never
/// becomes an unchecked slice or triggers an unbounded allocation.
pub(super) fn read_variable(
    mut size: impl FnMut() -> Result<usize, CliError>,
    mut read: impl FnMut(&mut [usize], usize) -> Result<usize, ReadFailure>,
) -> Result<Vec<u8>, CliError> {
    for _ in 0..ATTEMPTS {
        let bytes = size()?;
        if bytes > MAX_PROPERTY_BYTES {
            return Err(recording_error(
                "input device property exceeds its bounded envelope",
            ));
        }
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        match read(&mut storage, bytes) {
            Ok(returned) if returned <= bytes => {
                // SAFETY: storage is initialized, aligned, and returned <= the
                // checked requested extent. Copy only initialized storage bytes.
                return Ok(unsafe {
                    std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), returned)
                }
                .to_vec());
            }
            Ok(_) => {
                return Err(recording_error(
                    "macOS returned an inconsistent input property size",
                ))
            }
            Err(ReadFailure::Resized) => continue,
            Err(ReadFailure::Failed(error)) => return Err(error),
        }
    }
    Err(recording_error(
        "input device property changed size during discovery",
    ))
}

pub(super) fn parse_ids(bytes: &[u8]) -> Result<Vec<u32>, CliError> {
    if !bytes.len().is_multiple_of(4) || bytes.len() / 4 > MAX_DEVICES {
        return Err(recording_error("macOS returned a malformed device list"));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|id| u32::from_ne_bytes(id.try_into().unwrap()))
        .collect())
}

pub(super) fn parse_channels(bytes: &[u8]) -> Result<u32, CliError> {
    let malformed = || recording_error("macOS returned a malformed input stream configuration");
    let count =
        u32::from_ne_bytes(bytes.get(..4).ok_or_else(malformed)?.try_into().unwrap()) as usize;
    // AudioBuffer is two UInt32s followed by a pointer. AudioBufferList pads
    // its UInt32 count to the alignment of AudioBuffer on this platform.
    let pointer = std::mem::size_of::<usize>();
    let offset = 4usize.next_multiple_of(std::mem::align_of::<usize>());
    let stride = 8 + pointer;
    if count == 0 && bytes.len() == 4 {
        return Ok(0);
    }
    if count > 4096
        || offset
            .checked_add(count.checked_mul(stride).ok_or_else(malformed)?)
            .ok_or_else(malformed)?
            != bytes.len()
    {
        return Err(malformed());
    }
    let mut total = 0u32;
    for buffer in bytes[offset..].chunks_exact(stride) {
        total = total
            .checked_add(u32::from_ne_bytes(buffer[..4].try_into().unwrap()))
            .ok_or_else(malformed)?;
        if total > MAX_CHANNELS || buffer[8..].iter().any(|byte| *byte != 0) {
            return Err(malformed());
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap};

    #[derive(Clone, Default)]
    struct State {
        ids: Vec<u32>,
        default: u32,
        info: BTreeMap<u32, InputDeviceInfo>,
    }
    struct Fake {
        states: Vec<State>,
        step: usize,
        active: usize,
    }
    impl Fake {
        fn new(states: Vec<State>) -> Self {
            Self {
                states,
                step: 0,
                active: 0,
            }
        }
        fn one(state: State) -> Self {
            Self::new(vec![state])
        }
    }
    impl Provider for Fake {
        fn ids(&mut self) -> Result<Vec<u32>, CliError> {
            self.active = self.step.min(self.states.len() - 1);
            self.step += 1;
            Ok(self.states[self.active].ids.clone())
        }
        fn default_input(&mut self) -> Result<u32, CliError> {
            Ok(self.states[self.active].default)
        }
        fn info(&mut self, id: u32) -> Result<Option<InputDeviceInfo>, CliError> {
            Ok(self.states[self.active].info.get(&id).cloned())
        }
    }
    fn info(uid: &str) -> InputDeviceInfo {
        InputDeviceInfo {
            uid: uid.into(),
            name: "Same display name".into(),
            input_channels: 2,
            is_default: false,
            hardware_rate_hz: Some(44_100.0),
            available: true,
        }
    }
    fn state() -> State {
        State {
            ids: vec![1, 2, 3],
            default: 1,
            info: BTreeMap::from([(1, info("z-input")), (2, info("a-input"))]),
        }
    }

    #[test]
    fn empty_no_default_output_only_and_duplicate_names_are_unambiguous() {
        assert!(snapshot(&mut Fake::one(State::default()))
            .unwrap()
            .is_empty());
        assert!(resolve(&mut Fake::one(State::default()), None).is_err());
        let mut inputs = state();
        inputs.default = 0;
        let listed = snapshot(&mut Fake::one(inputs.clone())).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|input| input.info.uid.as_str())
                .collect::<Vec<_>>(),
            ["a-input", "z-input"]
        );
        assert!(listed.iter().all(|input| !input.info.is_default));
        assert!(resolve(&mut Fake::one(inputs.clone()), None).is_err());
        assert_eq!(
            resolve(&mut Fake::one(inputs.clone()), Some("a-input"))
                .unwrap()
                .object,
            2
        );
        assert!(resolve(&mut Fake::one(inputs), Some("output-only")).is_err());
        let listed = snapshot(&mut Fake::one(state())).unwrap();
        assert!(listed[1].info.is_default);
    }

    #[test]
    fn invalid_identifiers_availability_and_properties_fail_without_permission() {
        let mut cases = vec![
            (state(), "missing"),
            (
                State {
                    ids: vec![3],
                    ..State::default()
                },
                "output-only",
            ),
        ];
        let mut duplicate_ids = state();
        duplicate_ids.ids.push(1);
        cases.push((duplicate_ids, "z-input"));
        let mut duplicate_uids = state();
        duplicate_uids.info.get_mut(&2).unwrap().uid = "z-input".into();
        cases.push((duplicate_uids, "z-input"));
        let mut unavailable = state();
        let device = unavailable.info.get_mut(&1).unwrap();
        device.available = false;
        device.hardware_rate_hz = None;
        cases.push((unavailable.clone(), "z-input"));
        assert!(
            !snapshot(&mut Fake::one(unavailable)).unwrap()[1]
                .info
                .available
        );
        let mut nul = state();
        nul.info.get_mut(&1).unwrap().uid = "z\0input".into();
        cases.push((nul, "z\0input"));
        let mut bad_rate = state();
        bad_rate.info.get_mut(&1).unwrap().hardware_rate_hz = Some(f64::NAN);
        cases.push((bad_rate, "z-input"));
        for (state, uid) in cases {
            let authorized = Cell::new(false);
            let result = prepare_and_authorize(&mut Fake::one(state), 1, Some(uid), || {
                authorized.set(true);
                Ok(())
            });
            assert_eq!(result.unwrap_err().code, "E_RECORDING");
            assert!(!authorized.get());
        }
    }

    #[test]
    fn bounded_snapshot_retries_hotplug_and_refuses_unstable_or_failed_discovery() {
        let first = state();
        let mut changed = first.clone();
        changed.info.get_mut(&1).unwrap().uid = "new-input".into();
        let mut recovering = Fake::new(vec![
            first.clone(),
            changed.clone(),
            changed.clone(),
            changed.clone(),
        ]);
        assert_eq!(snapshot(&mut recovering).unwrap()[1].info.uid, "new-input");
        assert_eq!(recovering.step, 4);
        let mut unstable = Fake::new(vec![
            first.clone(),
            changed.clone(),
            first.clone(),
            changed.clone(),
            first,
            changed,
        ]);
        assert!(snapshot(&mut unstable).is_err());
        assert_eq!(unstable.step, 6);
        struct Failed;
        impl Provider for Failed {
            fn ids(&mut self) -> Result<Vec<u32>, CliError> {
                Err(recording_error("property read failed"))
            }
            fn default_input(&mut self) -> Result<u32, CliError> {
                unreachable!()
            }
            fn info(&mut self, _: u32) -> Result<Option<InputDeviceInfo>, CliError> {
                unreachable!()
            }
        }
        assert!(snapshot(&mut Failed)
            .unwrap_err()
            .message
            .contains("property read failed"));
    }

    #[test]
    fn permission_prompt_keeps_the_pinned_choice_and_rechecks_uniqueness() {
        let first = state();
        let mut new_default = first.clone();
        new_default.default = 2;
        let mut provider = Fake::new(vec![
            first.clone(),
            first.clone(),
            new_default.clone(),
            new_default,
        ]);
        let calls = Cell::new(0);
        let (selected, metadata) = prepare_and_authorize(&mut provider, 1, None, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(selected.object, 1);
        assert_eq!(metadata.uid, "z-input");
        assert_eq!(metadata.selection, "system-default");
        for mut changed in [first.clone(), first.clone(), first.clone(), first.clone()] {
            let case = calls.get();
            calls.set(case + 1);
            match case {
                1 => {
                    changed.info.remove(&1);
                }
                2 => {
                    changed.info.get_mut(&2).unwrap().uid = "z-input".into();
                }
                3 => {
                    changed.info.get_mut(&1).unwrap().hardware_rate_hz = Some(48_000.0);
                }
                _ => {
                    let old = changed.info.remove(&1).unwrap();
                    changed.ids = vec![2, 3, 4];
                    changed.info.insert(4, old);
                }
            }
            let mut provider =
                Fake::new(vec![first.clone(), first.clone(), changed.clone(), changed]);
            let authorized = Cell::new(false);
            assert!(
                prepare_and_authorize(&mut provider, 1, Some("z-input"), || {
                    authorized.set(true);
                    Ok(())
                })
                .is_err()
            );
            assert!(authorized.get());
        }
    }

    #[test]
    fn escaped_metadata_limit_is_checked_before_permission() {
        let uid = "\u{1}".repeat(4096);
        let mut inputs = state();
        inputs.info.get_mut(&1).unwrap().uid = uid.clone();
        let authorized = Cell::new(false);
        assert!(
            prepare_and_authorize(&mut Fake::one(inputs), 1, Some(&uid), || {
                authorized.set(true);
                Ok(())
            })
            .unwrap_err()
            .message
            .contains("bounded envelope")
        );
        assert!(!authorized.get());
    }

    #[test]
    fn property_reads_are_aligned_bounded_and_retry_size_races_finitely() {
        let calls = Cell::new(0);
        let result = read_variable(
            || Ok(if calls.get() == 0 { 4 } else { 8 }),
            |storage, capacity| {
                assert_eq!(storage.as_ptr() as usize % std::mem::align_of::<usize>(), 0);
                if calls.get() == 0 {
                    calls.set(1);
                    return Err(ReadFailure::Resized);
                }
                calls.set(2);
                // SAFETY: the test writes only the checked eight-byte capacity.
                let bytes = unsafe {
                    std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), capacity)
                };
                bytes[..4].copy_from_slice(&1u32.to_ne_bytes());
                bytes[4..].copy_from_slice(&2u32.to_ne_bytes());
                Ok(8)
            },
        )
        .unwrap();
        assert_eq!(parse_ids(&result).unwrap(), [1, 2]);
        assert_eq!(calls.get(), 2);
        assert!(read_variable(
            || Ok(MAX_PROPERTY_BYTES + 1),
            |_, _| panic!("oversized property must not be read")
        )
        .is_err());
        assert!(read_variable(|| Ok(4), |_, _| Ok(5)).is_err());
        let calls = Cell::new(0);
        assert!(read_variable(
            || Ok(4),
            |_, _| {
                calls.set(calls.get() + 1);
                Err(ReadFailure::Resized)
            }
        )
        .is_err());
        assert_eq!(calls.get(), 3);
        assert!(read_variable(
            || Ok(4),
            |_, _| Err(ReadFailure::Failed(recording_error("failed")))
        )
        .is_err());
        assert!(parse_ids(&[0, 1, 2]).is_err());
        assert!(parse_ids(&vec![0; (MAX_DEVICES + 1) * 4]).is_err());
    }

    #[test]
    fn stream_configuration_parsing_rejects_malformed_lengths_counts_and_channels() {
        let offset = 4usize.next_multiple_of(std::mem::align_of::<usize>());
        let stride = 8 + std::mem::size_of::<usize>();
        let mut bytes = vec![0; offset + 2 * stride];
        bytes[..4].copy_from_slice(&2u32.to_ne_bytes());
        bytes[offset..offset + 4].copy_from_slice(&1u32.to_ne_bytes());
        bytes[offset + stride..offset + stride + 4].copy_from_slice(&2u32.to_ne_bytes());
        assert_eq!(parse_channels(&bytes).unwrap(), 3);
        assert_eq!(parse_channels(&0u32.to_ne_bytes()).unwrap(), 0);
        for malformed in [&bytes[..3], &bytes[..bytes.len() - 1]] {
            assert!(parse_channels(malformed).is_err());
        }
        let mut malformed = bytes.clone();
        malformed[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_channels(&malformed).is_err());
        let mut malformed = bytes.clone();
        malformed[offset..offset + 4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_channels(&malformed).is_err());
        bytes[offset + 8] = 1;
        assert!(parse_channels(&bytes).is_err());
    }
}
