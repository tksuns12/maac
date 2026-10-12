# Production metering evidence

The approved single-stage four-times Annex 2 estimator passed all applicable
mono/stereo fixtures audited here through the public production analyzer.
Its identities are `maac.analysis.bs1770-5/2` and
`maac.truepeak.bs1770-5.annex2-4x/1`. This evidence does **not** certify metering
or claim full EBU Mode support. The historical two-stage 16× results below
explain the profile revision; that estimator is no longer selected.

## Provenance and reproduction

Audit date: 2026-09-09. Executor: Astra High. Implementation: Rust binary64,
ordered multiply/add, no FMA, checked nearest ties-to-even and preserved
subnormals. Tests ran on the repository's macOS host in release mode.

The official [EBU Loudness test set v05](https://tech.ebu.ch/publications/ebu_loudness_test_set)
was downloaded for internal technical evaluation through the
[Erlangen Gentoo mirror](https://ftp.uni-erlangen.de/gentoo/distfiles/de/ebu-loudness-test-setv05.zip).
Its contents identify v05, 30 March 2016. **© EBU.** Official audio remains in
`/tmp/ebu-v05`; no EBU audio is included in this repository. The
[EBU terms](https://tech.ebu.ch/files/live/sites/tech/files/shared/testmaterial/use%20of%20EBU%20AUDIO%20test%20sequences.pdf)
restrict use and redistribution.

The ZIP size and both hashes matched the upstream
[Gentoo libebur128 Manifest](https://raw.githubusercontent.com/gentoo/gentoo/master/media-libs/libebur128/Manifest):

```text
File: ebu-loudness-test-setv05.zip
Bytes: 91631421
BLAKE2B: 95a345b0700893ab96854c7563ecc9c667db30e8137352226d1f1ac7a666f6b08d1748d88ef11f72056d0a4bd513f365a1336de568ea45917a6ba9af5bc526ea
SHA512: 60d022fdac47ad0be2688411be9daecbff85da994d6fa4921bba6cffab841b081d8b15d9ce284ad2253efb686463450a84a0d19cb0bad7a934546cc52dd73771
```

The official EBU origin returned HTTP 403 to the initial download. The verified
mirror supplied identical distributable archive bytes; this is not a substituted
third-party test signal collection.

Commands (after privately downloading, verifying, and extracting the ZIP):

```sh
cargo test --lib production_analysis
cargo test --release --lib ebu_3341_prescribed_loudness -- --ignored --nocapture
MAAC_EBU_CORPUS=/tmp/ebu-v05 MAAC_METER_AUDIT_REPORT=/tmp/maac-meter-audit-4x.json \
  cargo test --release --lib ebu_official_corpus_audit -- --ignored --nocapture
cargo test --lib ebu_3341_prescribed_true_peak -- --nocapture
```

The final command now passes as an ordinary regression using the public 4×
analyzer. It no longer requires an ignored-test flag or accepts the historical
16× overread as the current result.

## Actual EBU corpus: integrated loudness

All applicable mono/stereo integrated-loudness cases in
[EBU Tech 3341 v4, Table 1](https://tech.ebu.ch/files/live/sites/tech/files/shared/tech/tech3341v4_0.pdf)
passed the published ±0.1 LUFS tolerance. Measurement reopened the original
48 kHz PCM16/PCM24 artifacts, including the two authentic programme excerpts.

| Case | Measured LUFS | Expected LUFS | Result |
| --- | ---: | ---: | --- |
| 1 | -22.953554851 | -23.0 | Pass |
| 2 | -32.959858908 | -33.0 | Pass |
| 3 | -23.014141653 | -23.0 | Pass |
| 4 | -23.014141653 | -23.0 | Pass |
| 5 | -22.979029141 | -23.0 | Pass |
| 7 | -22.986158805 | -23.0 | Pass |
| 8 | -22.997820241 | -23.0 | Pass |

Case 6 is multichannel and outside this mono/stereo profile. Cases 9–14 test
short-term/momentary functions that this delivery analyzer does not advertise.
Loudness range (Tech 3342) remains outside scope. The separately synthesized
full-duration cases 1–5 also passed; their Float32 encodings are independent
recreations of the published mathematical definitions, not original EBU files.

## Actual EBU corpus: true peak

The required tolerance is −0.4/+0.2 dB relative to the published expected value.
All original fixtures are 48 kHz. The public 4× analyzer passes all nine
applicable true-peak cases, including the four isolated transient/downsampling
phase cases. The historical 16× estimator failed cases 16 and 19.

| Case | Expected dBTP | Historical 16× dBTP | Historical result | Current 4× dBTP | Current result |
| --- | ---: | ---: | --- | ---: | --- |
| 15 | -6.0 | -5.964747309 | Pass | -6.000264939 | Pass |
| 16 | -6.0 | -5.640300929 | **Fail** | -5.955520147 | Pass |
| 17 | -6.0 | -6.276002948 | Pass | -6.295404745 | Pass |
| 18 | -6.0 | -5.965877078 | Pass | -6.007442487 | Pass |
| 19 | 3.0 | 3.369682147 | **Fail** | 3.054462934 | Pass |
| 20 | 0.0 | -0.016078268 | Pass | -0.130183129 | Pass |
| 21 | 0.0 | 0.011606094 | Pass | -0.078911987 | Pass |
| 22 | 0.0 | -0.121562115 | Pass | -0.204109672 | Pass |
| 23 | 0.0 | 0.011607195 | Pass | -0.078910896 | Pass |

The public 4× analyzer was run at 44.1, 48, and 96 kHz on the same normalized input
sample sequences. Its interpolation equation is independent of rate: the
fixtures' Fs/4, Fs/6, and Fs/8 definitions scale with the selected rate. The
reported peak values were identical at all three rates. This is a test of the
specified sample-domain algorithm across supported rate selections; it does
not imply the EBU supplied alternate-rate WAV files or that resampled versions
are official fixtures. Separately synthesized cases 15–19 exercise actual
10 ms tapers at each supported rate.

Independent Python periodic convolution of the published coefficients confirms
the cause: for the half-full-scale Fs/4, 45-degree tone, the first stage gives
−5.975908885 dBTP while the cascade gives −5.660664171 dBTP. The extra stage's
passband ripple adds about 0.315 dB here. The original-sample lower bound cannot
correct overreading. Rescaling or changing report tolerances would change the
contract and was not used.

## Approved profile revision

The implementation uses exactly **one** published Annex 2 four-times
interpolator with the existing 48 coefficients and ascending tap/phase order.
It preserves binary64 arithmetic, independent per-channel state, zero reset,
and zero extension. For N input frames, it includes all `4*(N+11)` output
frames. True peak is the maximum of that output's absolute sample values and
the original sample peak. There is no phase renormalization, additional
factor of four, or calibration gain.

The analyzer and true-peak identities above select the revised contract in
measurements and render identity. The numerical-environment coefficient
identity is `maac.analysis.bs1770-5/2:literal48k-rational-bilinear;annex2-table`.
A unit impulse's estimate is exactly 1 through the original-sample lower
bound, replacing the historical cascade's `67176863/67108864` expectation.
Loudness filters, gating, SRC, encoding, dither, and rendered audio are unchanged.

Passing these minimum requirements does not prove a continuous-frequency error
bound, performance on every possible signal, full EBU Mode compliance, or
listening quality. Official ITU BS.2217 applicability is audited separately;
no broader claim follows from EBU results alone.

## Official ITU BS.2217: all applicable mono/stereo files

All **19** applicable original 48 kHz mono/stereo WAVs from the
[official ITU compliance collection](https://www.itu.int/oth/R1102000001)
passed the file-based expected results and ±0.1 LUFS tolerance in
[Report ITU-R BS.2217-1 (2012)](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-BS.2217-1-2012-PDF-E.pdf).
These cover twelve frequency/level combinations, both gating boundaries, a
frequency sweep, and four authentic mono/stereo programme files. The files
remain private in `/tmp/itu-2217/audio`; multichannel files are outside scope.

| Original WAV | Measured LUFS | Expected LUFS | Result |
| --- | ---: | ---: | --- |
| `1770-2_Comp_23LKFS_25Hz_2ch.wav` | -22.993508679 | -23.0 | Pass |
| `1770-2_Comp_23LKFS_100Hz_2ch.wav` | -22.993717986 | -23.0 | Pass |
| `1770-2_Comp_23LKFS_500Hz_2ch.wav` | -22.993635534 | -23.0 | Pass |
| `1770-2_Comp_23LKFS_1000Hz_2ch.wav` | -22.993570081 | -23.0 | Pass |
| `1770-2_Comp_23LKFS_2000Hz_2ch.wav` | -22.992528329 | -23.0 | Pass |
| `1770-2_Comp_23LKFS_10000Hz_2ch.wav` | -22.993441415 | -23.0 | Pass |
| `1770-2_Comp_24LKFS_25Hz_2ch.wav` | -23.993548589 | -24.0 | Pass |
| `1770-2_Comp_24LKFS_100Hz_2ch.wav` | -23.993652987 | -24.0 | Pass |
| `1770-2_Comp_24LKFS_500Hz_2ch.wav` | -23.993381601 | -24.0 | Pass |
| `1770-2_Comp_24LKFS_1000Hz_2ch.wav` | -23.993773815 | -24.0 | Pass |
| `1770-2_Comp_24LKFS_2000Hz_2ch.wav` | -23.993477735 | -24.0 | Pass |
| `1770-2_Comp_24LKFS_10000Hz_2ch.wav` | -23.993688248 | -24.0 | Pass |
| `1770-2_Comp_AbsGateTest.wav` | -69.451555481 | -69.5 | Pass |
| `1770-2_Comp_RelGateTest.wav` | -10.028804577 | -10.0 | Pass |
| `1770-2_Comp_18LKFS_FrequencySweep.wav` | -17.990526806 | -18.0 | Pass |
| `1770-2 Conf Mono Voice+Music-23LKFS.wav` | -22.990611127 | -23.0 | Pass |
| `1770-2 Conf Stereo VinL+R-23LKFS.wav` | -22.978882451 | -23.0 | Pass |
| `1770-2 Conf Mono Voice+Music-24LKFS.wav` | -23.993561964 | -24.0 | Pass |
| `1770-2 Conf Stereo VinL+R-24LKFS.wav` | -23.975890681 | -24.0 | Pass |

Download provenance: each archive came directly from the official ITU URL
`https://www.itu.int/dms_pub/itu-r/oth/11/02/R1102000001<ID>ZIPM.zip`, where
`<ID>` is the following four-digit suffix. Every size matched the official
index; SHA-256 hashes below record the downloaded bytes. Extraction checked
ZIP CRC integrity. These locally computed hashes identify the audit inputs;
the ITU index does not publish independent SHA-256 values.

| ID | Bytes | Observed SHA-256 |
| --- | ---: | --- |
| 0002 | 8839132 | `e02715ec2f47f53190513d94837eb5d3f5d95f06e0aed7e4eaa7ba1dac62e644` |
| 0003 | 39321 | `f616b6b846a3fae805093b2b8403256ab16521b3e18ca0950e689a4b926e6093` |
| 0004 | 30914 | `b9aaa1f928e31a0af8c2edc2a15fe5ed9e5ee0bd0c9e97352ff72f1ab5406280` |
| 0005 | 24772 | `f15dd491c7c9f5359f7202fe659c35b15620b7c7e8044b43a685a32c952d59e8` |
| 0006 | 21968 | `aa00945bf2fb7593dfd4ee17f8eed00edabd7ce39ba434b8747b3e61a56b4296` |
| 0007 | 19200 | `fb8442e8d1d2e492834b5c9fb16a17ba5f5da229e360c36f1b648883e0ba3700` |
| 0008 | 19206 | `52a56dc8a9e29a1cdb33987bb6b073ed241702d7f4680fbd4444c5d228651cbc` |
| 0016 | 39307 | `15bd4c5a3f132363a2891d068b66c016c6aaecc252ecd1853ada4e744a803437` |
| 0017 | 30907 | `0d3c926d2109babd06ee52d8cdebab0f0b74eb0950e3c4dd7ebc9b89a31c91fb` |
| 0018 | 24766 | `3a2be685077d3a106335e4ca3968d6b64694ad1cd5a1463cdced4825a07fcc62` |
| 0019 | 21959 | `550aad69dba7aaca64334d94f40093db2b9ddc8e38c46c91a5c8a223f8c0c22e` |
| 0020 | 19202 | `894ae22077ced5dfa7c549842cf95bd964a9cfb97c5146e408c5b447059e9bd9` |
| 0021 | 19205 | `986da1dad4688843eae38376c4135003eeb8ae099ab6dd814756264308ace1de` |
| 0029 | 19394 | `a060a2c58bd29b4793faa6a9e6301e42052248a15a3004dfba7a9d5467f4d96e` |
| 0030 | 10505 | `f1958edae25b16f0df7b2b47013de9ad398278b8b2031a9be17cb880013a9ec6` |
| 0037 | 6568902 | `ae00f463a49183d257440343406490ebed65df2c8184ba1bf18ae4036e42acb8` |
| 0038 | 6534351 | `f880bad35e875652a75a2629ed545223eb44cfb782c2bdcd65ba17069058b52c` |
| 0039 | 13203678 | `c22f22651e4a4daace6678396ab3d6a7cbf1cb13731182b94cf141a6fdb604dd` |
| 0040 | 13112523 | `518c5d5a9184b28336949d57826b2be780d1ec3d3ab058a88e392e84282c9105` |

```sh
MAAC_ITU_CORPUS=/tmp/itu-2217/audio MAAC_ITU_AUDIT_REPORT=/tmp/maac-itu-audit-4x.json \
  cargo test --release --lib itu_official_loudness_corpus_audit -- --ignored --nocapture
```

The five larger archives initially exceeded short download allowances. HTTP
range resumption completed them from the same official URLs without replacing
or weakening the acceptance inputs. All applicable files were subsequently
measured; no download-based gap remains in this mono/stereo ITU corpus audit.

Fresh verification reran both official corpus audits together in the full
repository release build through the public `/2` analyzer:

```sh
MAAC_EBU_CORPUS=/tmp/ebu-v05 MAAC_METER_AUDIT_REPORT=/tmp/maac-meter-audit-4x.json \
MAAC_ITU_CORPUS=/tmp/itu-2217/audio MAAC_ITU_AUDIT_REPORT=/tmp/maac-itu-audit-4x.json \
  cargo test --release --lib official -- --ignored --nocapture
```

**Two audit tests passed**, with zero failures: all 19 ITU files, all seven
applicable EBU loudness files, and all nine EBU true-peak cases at each supported
rate selection. The ordinary analyzer suite passed **13 tests**, with three
explicitly opt-in audits. There is no current known-failure ignore. The EBU
report records the public analyzer/profile identities and each measured
`value_dbtp`/`pass` result; it does not validate a separate candidate path.

The remaining opt-in full-duration prescribed-loudness audit was also rerun:

```sh
cargo test --locked --offline --release --lib ebu_3341_prescribed_loudness -- --ignored --nocapture
```

It passed one test with zero failures. Cases 1–5 measured −22.993297102,
−32.993297041, −23.013868674, −23.013868674, and −22.978657437 LUFS,
respectively, all within ±0.1 LUFS. Thus all three opt-in analyzer audits have
observed success on the current public profile.
