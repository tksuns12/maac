# Production delivery acceptance

The installed Rust CLI passed the complete production example and delivery
publication checks on 2026-09-09. This is implementation evidence for native
processing, conversion, artifact analysis, and replay. The approved public 4×
true-peak analyzer passed the applicable audited ITU/EBU mono/stereo fixtures;
see [metering evidence](production-metering-evidence.md) for scope and the
historical 16× comparison. Full EBU Mode is outside this implementation.
No listening or professional sound-quality acceptance is claimed.

## Reproduce the installed boundary

With the repository's locked Cargo dependencies already cached, run:

```sh
python3 scripts/production_acceptance.py --profile song
```

The [runner](../scripts/production_acceptance.py) needs only the Python standard
library. It installs the actual CLI with `cargo install --locked --offline`
into a unique directory under `target/production-acceptance`, renders through
that installed executable, and independently reopens the resulting WAV bytes.
It preserves command logs, retained plans, audio, manifests, and a JSON report.
The latest local report is
[`target/production-acceptance/results.json`](../target/production-acceptance/results.json);
these generated artifacts are not committed.

The default runner profile is `song`. The full CD delivery charged
1,333,754,136 work units, below the default profile's 2,000,000,000 limit. The
archive charged 2,719,590,336 and requires the larger `song` allowance. The
4× analyzer's reduced work accounting includes its complete filter flush;
acceptance does not bypass resource checks.

Observed run: `run-rdfj8cds`, 2026-09-09 01:12:26–01:14:53 UTC, Astra High,
macOS 26.6.2 arm64, Python 3.14.4. The runner completed **157 checks**, including
nine delivery renders, and exited 0. Its fresh offline install completed in
52.10 seconds. Input and executable identities were:

```text
examples/production.maac
sha256:4289604a6c75fd72fce25a8d397a40dd4b15e7f503351454e7ab134ed176a2f0
production.schema.json
sha256:1f9f3623515e645834fd97ecb300cdf00c5e11f4dd260f5bd85a92d3af91568d
installed bin/maac
sha256:b8ef92b6ca9dd06494fa6fa2eb4ba41cbad586d4a638db6d8e27b29ef2f1ca7d
```

The executable hash records this build; it is not a cross-platform build
reproducibility promise. The report preserves exact measurements and full
manifest provenance, including converter certificates, dither identities and
seeds, selected ports, execution identity, and render identity.

This run also used the following optional baseline argument to compare the six WAVs
with the previous 16× macOS run. All six file and PCM hashes were identical.

```sh
python3 scripts/production_acceptance.py --profile song \
  --baseline-report target/production-acceptance/run-30_ee26p/results.json
```

The optional comparison uses a caller-selected report instead of imposing
platform-specific hashes on all installations. Both delivery render keys
changed with the analyzer identity; all six loudness and sample-peak records
were unchanged. Every current measurement and manifest identifies `/2` and
the 4× profile, with exactly `4*(N+11)` true-peak output frames.

## Original example artifacts

Both deliveries of the unchanged [example](../examples/production.maac)
succeeded. Every target contains the same complete seven-second project
interval: four seconds of score plus the declared three-second tail, rendered
as 336,000 engine frames at 48 kHz. Every artifact below is stereo.

| Delivery / target | Rate (Hz) | Frames | Encoding | File bytes | LUFS | Sample peak dBFS | Reported true peak dBTP |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| CD / master | 44,100 | 308,700 | PCM16 | 1,234,844 | -23.764452 | -14.163712 | -14.149842 |
| CD / processed_bass | 44,100 | 308,700 | PCM24 | 1,852,244 | -23.829638 | -18.958675 | -18.944885 |
| CD / pulse | 44,100 | 308,700 | PCM24 | 1,852,244 | -27.125072 | -15.820249 | -15.806501 |
| Archive / master | 96,000 | 672,000 | Float32 | 5,376,058 | -23.764223 | -14.164027 | -14.150270 |
| Archive / bass_with_room | 96,000 | 672,000 | Float32 | 5,376,058 | -24.465882 | -19.069497 | -19.055772 |
| Archive / pulse | 96,000 | 672,000 | Float32 | 5,376,058 | -27.125072 | -15.820246 | -15.806480 |

The CD master passed its illustrative −24…−22 LUFS interval and −1 dBFS/dBTP
maximums. Archive targets requested no limits and reported
`check_status = "not_requested"`. These are example policies, not universal
mastering targets. CD master uses deterministic TPDF seed 1, processed bass
uses seed 2, and the other four artifacts use no dither.

Final WAV SHA-256 hashes, including container bytes:

```text
release_cd.master.wav
b0c29ac8e5fabdbde32addd45fdafae5890e50059d578e639f689056d28da924
release_cd.processed_bass.wav
20bbffde60016cc7200622edf4971050dd9e0cb135c0f47af6e1041b6214ce13
release_cd.pulse.wav
34ab8888aa1c7a274f414e36065f3dc9cf555d434be5ab435ddfa75d298e7253
archive.master.wav
b2e20df8ab156bedb388fc7d37f7f998befc50a7ad383431cd75ee6a90421f0c
archive.bass_with_room.wav
b91a0abeae0d691a64bd1b714a5e9178c803406067dd28c7fbad3f683dd0c34c
archive.pulse.wav
f5b31a49053830bb6d571669ae15b30984092fe65f4bb69cf781aad7edc1cbdc
```

The independent WAV reader verified RIFF lengths, supported encodings,
interleaving, decoded frame counts, finite samples, nonzero audio, payload
hashes, and exact agreement between decoded sample peaks and the manifest.
Float32 artifacts include the WAVEFORMATEX `fmt` extension and `fact` chunk.
Loudness and true-peak numbers above are the implementation's measurements;
this runner does not supply an independent loudness or true-peak oracle.

## Failure, selection, and retained replay

All of the following passed at the installed CLI boundary:

- A temporary source copy changed only the CD master's loudness interval to
  −20…−18 LUFS. The master failed the minimum check, the process exited 1, and
  all three WAVs plus the complete manifest remained published. Every WAV was
  byte-identical to the successful original CD delivery. The repository
  example was not modified by this test.
- After compiling both policies, the runner deleted the entire temporary
  source package, including its schema. Both original deliveries rendered
  from retained JSON alone with identical WAV bytes. The failed-policy JSON
  also replayed with identical audio and the same failed check status.
- Selecting only `processed_bass` preserved its complete-delivery bytes,
  exercising the external pulse sidechain in the unmuted graph. Requesting
  targets in reversed order also preserved all three CD artifacts.
- Reusing an existing destination without `--force` returned
  `E_OUTPUT_EXISTS`; every existing file retained its hash, size, and
  modification time. Explicit `--force` replay published identical audio and
  still returned the failed loudness result.
- Each output directory contained exactly its expected WAVs and complete
  manifest, with no staging files left behind. Publication is atomic per
  artifact; this does not establish a transaction across multiple files.

## Numerical environment and qualification scope

The manifest records analyzer `maac.analysis.bs1770-5/2`, true-peak profile
`maac.truepeak.bs1770-5.annex2-4x/1`, and converter `maac.src.kaiser/1`.
Arithmetic is ordered binary64 multiply/add, nearest ties-to-even, no FMA,
and no denormal flushing. The measured platform is `aarch64-macos-unix`;
logarithms use the platform implementation behind Rust `f64::log10`.
The analyzer build field is explicitly `build-id-unavailable`; the separately
recorded executable digest above identifies this acceptance binary.

The separate [official metering audit](production-metering-evidence.md)
passed all 19 applicable ITU mono/stereo loudness files and all seven applicable
EBU integrated-loudness cases. The public 4× analyzer also passed all nine
applicable EBU true-peak cases at each supported rate selection. The historical
16× cascade overread cases 16 and 19; it is no longer selected. These bounded
fixture results do not establish full EBU Mode, a continuous-frequency error
bound, acoustic realism, or listening quality for the reverb or other DSP.

The following integration checks were also observed on this implementation:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Pass |
| `cargo test --locked --offline` | 424 passed, 0 failed, 3 ignored |
| `python3 scripts/acceptance.py` | 38 checks passed through the freshly installed legacy CLI; `target/acceptance/results.json` reports `ok: true` |
| Pinned Python `check_production.py` | 8 valid, 77 invalid, 60 arithmetic fixtures; schema pin checked; no tracked writes |
| Legacy `check_spec.py` in a disposable copy | Previously passed; checker, grammar, and all three snapshots are unchanged in this revision |
| SRC generator `--verify`, normal Python and `python3 -O` | Both passed before this analyzer revision, including full recomputation; generator, certificate, and table bytes are unchanged |
| SRC generator `--self-test`, normal Python and `python3 -O` | Both pass; five invalid proof/verification cases rejected |
| `python3 -m py_compile scripts/production_acceptance.py` | Pass |

The three ignored Rust tests are explicit metering audits. The fresh official
corpus tests passed (two tests), as did the separately rerun full-duration
prescribed-loudness audit (one test). All three therefore have observed
current-profile success, documented in the metering report. Prescribed true-peak
cases now pass through the public analyzer as ordinary tests; there is no
current known-failure ignore.
