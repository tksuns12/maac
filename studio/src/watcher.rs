//! Project directory watcher for live collaboration with external editors and AI agents.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct ProjectWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<Instant>,
}

impl ProjectWatcher {
    pub fn watch(project_root: &Path) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            let relevant_kind = matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            );
            if !relevant_kind {
                return;
            }
            let touches_source = event.paths.iter().any(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("maac")
                            || ext.eq_ignore_ascii_case("wav")
                            || ext.eq_ignore_ascii_case("pcm")
                    })
            });
            if touches_source {
                let _ = tx.send(Instant::now());
            }
        })
        .map_err(|err| format!("cannot start file watcher: {err}"))?;

        watcher
            .watch(project_root, RecursiveMode::Recursive)
            .map_err(|err| format!("cannot watch {}: {err}", project_root.display()))?;

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Drain all queued filesystem notifications and return the latest event
    /// timestamp, if any arrived since the last poll.
    pub fn poll_latest(&self) -> Option<Instant> {
        let mut latest = None;
        while let Ok(at) = self.rx.try_recv() {
            latest = Some(at);
        }
        latest
    }
}
