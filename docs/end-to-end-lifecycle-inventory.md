# End-to-end lifecycle inventory

**Snapshot:** 2026-09-28

**Audience:** contributors inspecting existing MaaC lifecycle capabilities. Use this
inventory to identify which bounded paths exist and what evidence is still
needed before claiming a complete production workflow.

This is an implementation inventory, not a product-readiness statement or the
active language backlog. The broader compose-through-reopen product proposal
is deferred. See the [language plan](language-specification-plan.md#current-priorities)
for active sequencing, the [broader product roadmap](end-to-end-production-plan.md)
for deferred lifecycle gates,
the [capability matrix](capabilities.md) for implemented scope, and
[release readiness](release-readiness.md) for publication status.

## Status labels

- **supported** — a bounded executable behavior and supporting evidence exist.
- **partial** — some useful behavior exists, but a named lifecycle obligation
  remains open.
- **importer-only** — the behavior is available at a particular import
  boundary; it does not establish a recording or general media workflow.
- **statically specified** — a wire or behavioral contract is documented and
  can be structurally checked, but the requested runtime or lifecycle path is
  not implemented.
- **missing** — no implemented path or sufficient static contract is evidenced
  for the capability described in the row.

Statuses describe the exact bounded capability named in each row. They do not
imply complete MaaC/1 conformance, professional quality, or end-to-end
production readiness.

## Lifecycle map

| Lifecycle area | Status | Current boundary and evidence | Possible product follow-up (deferred) |
| --- | --- | --- | --- |
| Authoring and offline composition | supported | MaaC source can express finite note and hit patterns, explicit arrangements, tempo and meter, a bounded set of instruments and processors, and reset-state offline rendering. The implementation remains a subset of the specification. See [capabilities](capabilities.md), the [Core Audio audit](core-audio-conformance-audit.md), and [release readiness](release-readiness.md). | Expand only against separately accepted contracts; retain explicit unsupported-feature errors. |
| Sample-kit input | supported | Core kits consume pinned raw float32 mono/stereo PCM at native rate for one-shot hits, with interpolation and natural tails. This is not a pitched sampled-instrument library. See [capabilities](capabilities.md), the [kit contract](core-kit.md), and [kit validation](core-kit-validation.md). | Add pinned sampled instruments, articulation maps, and larger open sound-library coverage under Phase 5. |
| WAV import and source-crop import | importer-only | `maac import-wav` accepts bounded mono/stereo PCM16/24/32 or IEEE float32 RIFF WAV and can import a nonempty source-frame crop as native float32 PCM with provenance. `--disk-media` permits a file-backed decoded crop up to 1 GiB under the disk-media profile; retaining and verifying the original WAV is opt-in. See [media import](media-import.md), [import implementation](../src/media_import.rs), and [CLI evidence](../tests/media_import_cli.rs). | Add broader decoders, recording metadata, and streaming or resource behavior beyond this bounded disk path. |
| Recording from devices | partial | [`maac record`](recording.md) provides bounded macOS default or exact UID-selected input capture as delivered 48 kHz mono Float32, retained WAV provenance, atomic project publication, and failure/cancellation cleanup. Opt-in [monitoring](input-monitoring.md) routes dry channel 1 to outputs on the same explicit 48 kHz duplex device. Synthetic lifecycle evidence does not establish physical microphone or monitoring acceptance. | Exercise permission, hardware capture and monitoring deliberately; add multichannel capture, separate output-device selection, measured latency, and recovery. |
| Arranged audio crops and fades | supported | Rate-mode clips can use half-open source-frame slices, reverse/speed, gain, and linear or equal-power fades. Warp-rate clips add explicit musical source-frame anchors. `patch --disk-media` applies source-preserving Protocol 2 crop, placement, and fade edits with conflicts and inverses against captured media. See [audio clips](audio-clips.md), [warp-rate clips](warp-rate.md), and their [validation](audio-clips-validation.md). | Add practical multi-lane editing and broader comp workflows; preserve explicit score/seconds semantics. |
| Rendered excerpt crops | supported | `maac render` can write a reset-origin half-open WAV frame range. It still prepares and executes the complete plan through the selected end, so stateful history before the excerpt is preserved. This does not provide seek, realtime preview, source cropping, or shortened delivery. See [range export](render-range.md), [validation](render-range-validation.md), and [implementation](../src/export.rs). | Keep this gate bounded to final-WAV excerpts; implement any later seek path with explicit prehistory/state restoration. |
| Disk-backed native PCM | supported | `check` and `build --disk-media` verify hash-pinned native PCM up to the documented per-file and aggregate budgets, snapshot it, and render through bounded cache pages. Artifact disk storage is a separate opt-in path for PCM already embedded in the plan. Neither path creates an external-media retained plan or general streaming archive. See [disk media](disk-media.md), [audio storage](audio-clips.md#runtime-pcm-storage), and [CLI evidence](../tests/disk_media_cli.rs). | Extend media and archive coverage for practical recording lengths, crop reads, and failure/resource behavior without describing bounded limits as streaming. |
| Audition and playback | partial | [`maac play`](playback.md) renders source or a retained plan to a private Float32 WAV and uses macOS system-default output. It supports source disk media, existing profiles, interruption, cleanup, and explicit backend errors. | Add device selection, low-latency transport, monitoring, measured latency, other platforms, and listening acceptance. |
| Processor latency and alignment | partial | The engine reports technical latency for supported core processors; `core.delay/1` reports its configured frame delay, while the current native production EQ, compressor, and reverb declare zero technical latency. Generic lock and descriptor contracts carry declared fixed latency. Rendering does not automatically compensate delay, and device/input latency capture and multitrack alignment are absent. See [core delay](core-delay.md), [production](production.md), [generic interchange](generic-interchange.md), and [roadmap constraints](end-to-end-production-plan.md). | Define capture-origin and alignment policy, then demonstrate latency-aligned recording/playback and render behavior on a representative path. |
| Alternate takes and comping | partial | `maac.takes/1` records alternate mono/stereo assets, a shared physical origin, and disjoint comp regions checked against explicit native rate clips. [`maac.takes/2`](grouped-takes.md) adds complete microphone-file lane sets with individual source origins and one synchronized selection. Protocol 2 switches selections atomically with conflicts and inverses; archives preserve inactive alternates and selection history after relocation. See [takes and comping](takes-and-comping.md). | Connect recorded projects to explicit take groups; add low-latency audition controls, broader comp editing, and representative listening acceptance. |
| Automation and control | supported | Authored automation supports score and seconds clocks with step, linear, and exponential interpolation at declared parameter rates; explicit modulation adds bounded control sources and event/reset capture. Clip transport fields themselves are not automation targets. See [capabilities](capabilities.md), [core modulation](core-modulation.md), and [production](production.md). | Add producer-facing automation editing and ensure its authored state, target mapping, and invalidation survive archive and reopen workflows. |
| Audio routing and sidechains | supported | Mono/stereo audio routing is explicit in the graph; there are no implicit mixers or channel conversions. The native compressor accepts one explicitly connected external sidechain. Track grouping alone creates no routing. See [capabilities](capabilities.md), [production](production.md), and the [production example](../examples/production.maac). | Demonstrate routing, shared effects, channel roles, and any desired send conventions inside the representative mix workload. |
| Mix and master processing | partial | An experimental native production extension supplies project-level EQ, linked peak compression, eight-delay reverb, and explicit master/stem capture. Its bounded tests and metering evidence do not establish producer acceptance, full EBU Mode, or professional sound quality. See [production](production.md), [production delivery evidence](production-delivery.md), and [metering evidence](production-metering-evidence.md). | Complete quality profiles, alignment and channel-role policy, delivery variants, invalidation-aware intermediate assets, listening review, and producer acceptance. |
| Named audio delivery | supported | Named outputs support complete-graph masters and stems, 44.1/48/96 kHz conversion, Float32/PCM24/PCM16 WAV, explicit dither, and measured final artifacts with requested-limit status. These are bounded implementation results, not universal mastering targets. See [capabilities](capabilities.md), [production](production.md), and [delivery evidence](production-delivery.md). | Exercise multiple outputs within the representative end-to-end workload and retain a manifest of outputs and decisions. |
| Interchange and DAW handoff | partial | Generic lock construction/verification and strict external descriptor/dependency inspection exist, and the MIDI 1.0 SMF adapter emits bounded exports and loss reports. Bounded built-in/core generic rendering and an explicit native ABI host also exist; mixed external/core graph execution, aligned-stem handoff, notation, and DAW-session adapters remain deferred. See [generic interchange](generic-interchange.md), [capabilities](capabilities.md), and the [Core Audio audit](core-audio-conformance-audit.md). | Name a receiver and version; inspect preserved fields and report exact losses before accepting a DAW profile. |
| External processor execution and generic rendering | partial | Lock, descriptor, dependency, state, latency, and permission contracts have structural validation and verification APIs. `maac::generic_render` renders verified locks for already-resolved built-in/core plans; `maac::external_host` and `maac::external_native` explicitly host the published native ABI; `maac::generic_external_render` executes exactly one output-only external generator. Mixed external/core graph execution, external inputs/events, automated external parameters, and sandboxed hosting remain deferred. See [generic interchange](generic-interchange.md), [native external ABI](native-external-abi-v1.md), and the [Core Audio audit](core-audio-conformance-audit.md). | Accept a host/ABI and sandbox contract, implement execution against it, and demonstrate faithful rendering or explicit failure for unavailable required processors. |
| Editable archive and history | partial | Native archive versions 1–12 capture the supported source/media dependency closure, selected retained WAV imports, explicit checkpoints and bounded edit journals, optional freezes, and optional derived native processor context. External executable modules and non-null external state, broader invalidation, overlapping frozen branches, arbitrary automatic history, and a complete producer archive remain open. See [editable archives](editable-archive.md), [archive implementation](../src/archive.rs), and [archive verification record](verification.md). | Preserve all render-affecting state and dependencies, broaden invalidation/history coverage, and prove package verification after relocation. |
| Reopen after import, relocation, or archive unpack | partial | A verified WAV-import project can move and compile/build without the original path; an archive can verify/unpack selected checkpoints and reopen supported source/media closure. Built-in imports still require the matching registry. These are bounded reopen paths, not a full clean-environment production reopen. See [media import](media-import.md), [editable archives](editable-archive.md), and [verification](verification.md). | Reopen the final editable project in a freshly provisioned offline environment using only the open baseline; verify every pin and preserve every render-affecting choice. |

## Cross-cutting findings

- Media is currently split across embedded raw PCM, imported WAV crops, and an
  opt-in disk-media profile for hash-pinned native PCM. File-backed WAV import
  permits larger selected crops, but a small crop still snapshots and reads the
  entire WAV once; the disk-media path scans all samples before rendering.
  Neither behavior is general-purpose streaming.
- The only crop contracts currently evidenced are source-frame slices in
  imported/arranged audio and reset-correct final-WAV range export. Realtime
  seeking, speed-up preview, and arbitrary DSP-state restoration remain absent.
- Processor latency is declared or reported at specific boundaries. No general
  device-latency capture, automatic compensation, or multi-recording alignment
  workflow is evidenced.
- WAV import does not infer take membership or alignment. The separate
  take capabilities supply explicitly authored alternate membership,
  common origins, synchronized microphone-file lanes, and selection history;
  bounded device capture creates a separate imported project and does not infer
  group membership or alignment.
- Automation and routing are authored source/graph features, but the
  end-to-end package must still demonstrate that every choice survives a
  representative archive, clean reopen, and final delivery.
- Current delivery and archive slices have focused execution and byte-level
  evidence. They have not been combined into the required production workload
  with recorded audio, multiple takes, comping, mix/master changes, invalidation,
  and a fresh offline reopen.

## Deferred product acceptance gates

These are broader workflow evaluations if that product track resumes. They do
not select the next language task. Existing partial implementations in the
table above must be credited before defining any follow-up slice.

1. **Media and capture:** Give imported and recorded assets explicit format,
   rate, channel, frame, source, and provenance identities. Add bounded disk or
   streaming access, crop/error behavior, device capture, and dropout recovery.
2. **Latency, takes, and edits:** Extend the bounded common-origin and comp
   contracts to device latency, monitoring versus render paths, broader
   source-addressed edits, and listening acceptance.
3. **Dependency-complete archive:** Preserve authored source, media, library
   pins, processor identity and state, latency/alignment, automation, routing,
   delivery policy, history, and freeze boundaries. Verify selective
   invalidation and reject missing dependencies explicitly.
4. **Representative production and reopen:** Run a multi-minute project with
   synthesizers, a sampled instrument, recorded audio, multiple takes,
   automation, routing, mix/master processing, and multiple deliveries. Edit
   an upstream dependency, check which renders invalidate, then verify and
   reopen the final archive offline in a freshly provisioned open-baseline
   environment.
5. **Human acceptance:** Review listening quality and producer workflow on the
   representative workload. Automated structure, replay, metering, and file
   checks do not replace this gate.

These deferred gates follow the [end-to-end production plan](end-to-end-production-plan.md).
The inventory records concrete bounded evidence to guide that work; it does not
declare the full lifecycle complete.
