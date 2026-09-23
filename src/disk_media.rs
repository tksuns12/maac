//! Opt-in project media snapshots outside the standalone artifact wire format.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::bundle::SourceBundle;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::dsp::{self, AudioStorageMode, DspEngine};
use crate::plan::{OutputSettings, PlanLimits};
use crate::plan_artifact::PlanArtifact;

pub(crate) const MAX_DISK_MEDIA_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const MAX_DISK_MEDIA_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct DiskAsset {
    pub(crate) file: Arc<File>,
    pub(crate) bytes: u64,
    pub(crate) hash: String,
}

impl PartialEq for DiskAsset {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.hash == other.hash && Arc::ptr_eq(&self.file, &other.file)
    }
}
impl Eq for DiskAsset {}

impl DiskAsset {
    pub(crate) fn snapshot(
        mut source: File,
        path: &str,
        expected: &str,
    ) -> Result<Self, Diagnostics> {
        let size = source
            .metadata()
            .map_err(|e| {
                fail(
                    DiagnosticCode::Asset,
                    format!("cannot inspect media `{path}`: {e}"),
                )
            })?
            .len();
        if size > MAX_DISK_MEDIA_FILE_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                format!("media `{path}` exceeds 1 GiB file limit"),
            ));
        }
        let mut private = tempfile::tempfile().map_err(|e| {
            fail(
                DiagnosticCode::Asset,
                format!("cannot create private media snapshot: {e}"),
            )
        })?;
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        let mut pending = [0u8; 4];
        let mut pending_len = 0usize;
        let mut block = [0u8; 64 * 1024];
        loop {
            let count = source.read(&mut block).map_err(|e| {
                fail(
                    DiagnosticCode::Asset,
                    format!("cannot read media `{path}`: {e}"),
                )
            })?;
            if count == 0 {
                break;
            }
            bytes = bytes
                .checked_add(count as u64)
                .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "media length overflow"))?;
            if bytes > MAX_DISK_MEDIA_FILE_BYTES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    format!("media `{path}` exceeds 1 GiB file limit"),
                ));
            }
            digest.update(&block[..count]);
            for &byte in &block[..count] {
                pending[pending_len] = byte;
                pending_len += 1;
                if pending_len == 4 {
                    if !f32::from_le_bytes(pending).is_finite() {
                        return Err(fail(
                            DiagnosticCode::Asset,
                            format!("media `{path}` has a nonfinite PCM sample"),
                        ));
                    }
                    pending_len = 0;
                }
            }
            private.write_all(&block[..count]).map_err(|e| {
                fail(
                    DiagnosticCode::Asset,
                    format!("cannot snapshot media `{path}`: {e}"),
                )
            })?;
        }
        if pending_len != 0 {
            return Err(fail(
                DiagnosticCode::Asset,
                format!("media `{path}` byte length is not a multiple of four"),
            ));
        }
        let hash = format!("sha256:{:x}", digest.finalize());
        if hash != expected {
            return Err(fail(
                DiagnosticCode::Hash,
                format!("media `{path}` expected `{expected}`, but has `{hash}`"),
            ));
        }
        private.flush().map_err(|e| {
            fail(
                DiagnosticCode::Asset,
                format!("cannot flush media snapshot: {e}"),
            )
        })?;
        private.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Asset,
                format!("cannot rewind media snapshot: {e}"),
            )
        })?;
        Ok(Self {
            file: Arc::new(private),
            bytes,
            hash,
        })
    }
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

/// A pinned, dependency-complete project load with private native PCM snapshots.
pub(crate) struct DiskMediaProject {
    bundle: SourceBundle,
    assets: BTreeMap<String, Arc<DiskAsset>>,
}

impl DiskMediaProject {
    pub(crate) fn load(entry: &Path, project_root: &Path) -> Result<Self, Diagnostics> {
        let root =
            crate::bundle_fs::ProjectRoot::open_pinned(project_root, cap_std::ambient_authority())?;
        Self::load_in_root(entry, &root)
    }

    pub(crate) fn load_in_root(
        entry: &Path,
        root: &crate::bundle_fs::ProjectRoot,
    ) -> Result<Self, Diagnostics> {
        let (bundle, assets) = crate::bundle_fs::load_disk_media_bundle_in_root(entry, root)?;
        Ok(Self { bundle, assets })
    }

    pub(crate) fn build_with_limits(
        &self,
        limits: &PlanLimits,
    ) -> Result<DiskMediaPlan, Diagnostics> {
        let artifact =
            crate::compiler::compile_disk_media_artifact(&self.bundle, &self.assets, limits)?;
        Ok(DiskMediaPlan {
            artifact,
            limits: *limits,
        })
    }

    pub(crate) fn bundle(&self) -> &SourceBundle {
        &self.bundle
    }

    pub(crate) fn disk_assets(&self) -> &BTreeMap<String, Arc<DiskAsset>> {
        &self.assets
    }
}

pub(crate) struct DiskMediaPlan {
    artifact: PlanArtifact,
    limits: PlanLimits,
}

impl DiskMediaPlan {
    pub(crate) fn output(&self) -> &OutputSettings {
        self.artifact.view().output
    }

    pub(crate) fn stats(&self) -> (usize, usize, usize) {
        let mut notes = 0;
        let mut hits = 0;
        for event in self.artifact.view().events() {
            match event.kind {
                crate::plan::EventKind::Note { .. } => notes += 1,
                crate::plan::EventKind::Hit { .. } => hits += 1,
                _ => {}
            }
        }
        (notes, hits, self.artifact.audio_clip_count())
    }

    pub(crate) fn render<F>(&self, callback: F) -> dsp::Result<()>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        DspEngine::new_artifact_with_limits_and_storage_mode(
            &self.artifact,
            &self.limits,
            AudioStorageMode::Disk,
        )?
        .render(callback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(root: &Path, samples: &[f32]) -> std::path::PathBuf {
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        fs::write(root.join("sample.pcm"), &bytes).unwrap();
        let source = format!(
            r#"maac 1;
project p {{ score=[0q,1/4q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}
tempo clock {{ points=[(0q,120bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames={}; }}
audio clip {{ asset=&sample; at=0q; source=[0frame,{}frame]; mode=rate; }}
"#,
            crate::bundle::sha256_digest(&bytes),
            samples.len(),
            samples.len()
        );
        let entry = root.join("main.maac");
        fs::write(&entry, source).unwrap();
        entry
    }

    #[test]
    fn snapshot_survives_source_replacement_and_reads_across_pages() {
        let temp = tempfile::tempdir().unwrap();
        let samples: Vec<f32> = (0..5000).map(|i| (i % 17) as f32 / 17.).collect();
        let entry = project(temp.path(), &samples);
        let project = DiskMediaProject::load(&entry, temp.path()).unwrap();
        fs::remove_file(&entry).unwrap();
        fs::remove_file(temp.path().join("sample.pcm")).unwrap();
        let plan = project.build_with_limits(&PlanLimits::default()).unwrap();
        assert_eq!(plan.stats(), (0, 0, 1));
        let mut rendered = Vec::new();
        plan.render(|frame| {
            rendered.push(frame[0]);
            Ok(())
        })
        .unwrap();
        assert_eq!(rendered.len(), plan.output().total_frames as usize);
        for index in [0, 1, 4095, 4096, 4799] {
            assert!((rendered[index] - f64::from(samples[index])).abs() < 1e-6);
        }
    }

    #[test]
    fn disk_media_rejects_nonfinite_pcm_and_bad_pin() {
        let temp = tempfile::tempdir().unwrap();
        let entry = project(temp.path(), &[1.0, 2.0]);
        fs::write(temp.path().join("sample.pcm"), f32::NAN.to_le_bytes()).unwrap();
        assert_eq!(
            DiskMediaProject::load(&entry, temp.path())
                .err()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .code,
            DiagnosticCode::Asset
        );
        fs::write(
            temp.path().join("sample.pcm"),
            [1.0f32, 3.0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            DiskMediaProject::load(&entry, temp.path())
                .err()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .code,
            DiagnosticCode::Hash
        );
    }

    #[test]
    fn disk_media_requires_composition_entrypoint() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("library.maac");
        fs::write(&entry, "maac 1;").unwrap();
        let project = DiskMediaProject::load(&entry, temp.path()).unwrap();
        let error = project
            .build_with_limits(&PlanLimits::default())
            .err()
            .unwrap();
        assert_eq!(
            error.iter().next().unwrap().code,
            DiagnosticCode::Capability
        );
    }

    #[test]
    fn repeated_media_path_checks_every_declared_hash() {
        let temp = tempfile::tempdir().unwrap();
        let entry = project(temp.path(), &[1.0, 2.0]);
        let stale = format!(
            "asset stale {{ kind=audio; path=\"sample.pcm\"; hash=\"sha256:{}\"; format=\"pcm_f32le_interleaved/1\"; rate=48000Hz; channels=1; frames=2; }}\n",
            "0".repeat(64)
        );
        let mut source = fs::read_to_string(&entry).unwrap();
        source.push_str(&stale);
        fs::write(&entry, source).unwrap();
        let error = DiskMediaProject::load(&entry, temp.path()).err().unwrap();
        assert_eq!(error.iter().next().unwrap().code, DiagnosticCode::Hash);
    }
}
