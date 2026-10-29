use clean_my_mac::{
    CandidateSpec, ScanResult, clean_candidate, default_candidates, display_path, scan_candidate,
    volume_usage,
};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    id: String,
    name: String,
    path: String,
    description: String,
    kind: String,
    allocated_bytes: u64,
    reclaimable_bytes: u64,
    selectable: bool,
    status: String,
    errors: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    items: Vec<Item>,
    volume: Option<Volume>,
    volume_error: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ScanProgress {
    Started {
        items: Vec<Item>,
    },
    Volume {
        volume: Option<Volume>,
        error: Option<String>,
    },
    Scanning {
        id: String,
        completed: usize,
        total: usize,
    },
    Item {
        item: Item,
        completed: usize,
        total: usize,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    reclaimed_bytes: u64,
    failures: Vec<String>,
}

pub struct Scanner {
    home: PathBuf,
    eligible: HashSet<String>,
}

fn selectable(spec: &CandidateSpec, scan: &ScanResult) -> bool {
    spec.kind.selectable() && scan.accessible() && !scan.open_handles && scan.reclaimable_bytes > 0
}

impl Scanner {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            eligible: HashSet::new(),
        }
    }

    pub fn scan(
        &mut self,
        mut emit: impl FnMut(ScanProgress) -> Result<(), String>,
    ) -> Result<Snapshot, String> {
        self.eligible.clear();
        let specs = default_candidates(&self.home);
        let pending = specs
            .iter()
            .map(|spec| Item {
                id: spec.id.to_owned(),
                name: spec.name.to_owned(),
                path: display_path(&spec.path, &self.home)
                    .to_string_lossy()
                    .into(),
                description: spec.description.to_owned(),
                kind: spec.kind.label().to_owned(),
                allocated_bytes: 0,
                reclaimable_bytes: 0,
                selectable: false,
                status: "待扫描".into(),
                errors: Vec::new(),
            })
            .collect();
        emit(ScanProgress::Started { items: pending })?;
        let (volume, volume_error) = match volume_usage() {
            Ok(usage) => (
                Some(Volume {
                    total_bytes: usage.total_bytes,
                    used_bytes: usage.used_bytes,
                    available_bytes: usage.available_bytes,
                }),
                None,
            ),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        emit(ScanProgress::Volume {
            volume: volume.clone(),
            error: volume_error.clone(),
        })?;
        let mut eligible = HashSet::new();
        let mut items = Vec::with_capacity(specs.len());
        for (index, spec) in specs.iter().enumerate() {
            emit(ScanProgress::Scanning {
                id: spec.id.to_owned(),
                completed: index,
                total: specs.len(),
            })?;
            let scan = scan_candidate(spec);
            let selectable = selectable(spec, &scan);
            if selectable {
                eligible.insert(spec.id.to_owned());
            }
            let status = if !spec.kind.selectable() {
                "受保护"
            } else if !scan.accessible() {
                "无法读取"
            } else if scan.open_handles {
                "正在使用"
            } else if scan.reclaimable_bytes == 0 {
                "无可清理内容"
            } else {
                "可清理"
            };
            let item = Item {
                id: spec.id.to_owned(),
                name: spec.name.to_owned(),
                path: display_path(&spec.path, &self.home)
                    .to_string_lossy()
                    .into(),
                description: spec.description.to_owned(),
                kind: spec.kind.label().to_owned(),
                allocated_bytes: scan.allocated_bytes,
                reclaimable_bytes: scan.reclaimable_bytes,
                selectable,
                status: status.to_owned(),
                errors: scan.errors,
            };
            emit(ScanProgress::Item {
                item: item.clone(),
                completed: index + 1,
                total: specs.len(),
            })?;
            items.push(item);
        }
        self.eligible = eligible;
        Ok(Snapshot {
            items,
            volume,
            volume_error,
        })
    }

    pub fn clean(&mut self, ids: &[String], confirmed: bool) -> Result<CleanupReport, String> {
        if !confirmed || ids.is_empty() {
            return Err("请先选择项目并确认永久删除。".into());
        }
        let specs = default_candidates(&self.home);
        // Validate the entire request before the first deletion. Paths never come from the webview.
        let mut seen = HashSet::new();
        let selected: Vec<_> = ids
            .iter()
            .map(|id| {
                if !seen.insert(id) {
                    return Err(format!("重复的清理项目：{id}"));
                }
                let spec = specs
                    .iter()
                    .find(|spec| spec.id == id)
                    .ok_or_else(|| format!("未知的清理项目：{id}"))?;
                if !self.eligible.contains(id) {
                    return Err(format!("请重新扫描后选择可清理项目：{}", spec.name));
                }
                Ok(spec)
            })
            .collect::<Result<_, String>>()?;

        self.eligible.clear();
        let mut report = CleanupReport {
            reclaimed_bytes: 0,
            failures: Vec::new(),
        };
        for spec in selected {
            // Recheck permissions/open files immediately before cleanup, not just at scan time.
            let current = scan_candidate(spec);
            if !current.accessible() || current.open_handles || !spec.kind.selectable() {
                report.failures.push(format!(
                    "{}：目录无法读取或正在使用，已跳过。{}",
                    spec.name,
                    current.errors.join("；")
                ));
                continue;
            }
            match clean_candidate(spec, &self.home) {
                Ok(bytes) => {
                    report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(bytes);
                }
                Err(error) => report.failures.push(format!("{}：{error:#}", spec.name)),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cleanup_requires_scan_confirmation_and_valid_unique_ids() {
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("Library/Caches/Google");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("data"), [1; 4096]).unwrap();
        let mut scanner = Scanner::new(home.path().to_owned());
        let ids = vec!["google-cache".to_owned()];
        assert!(scanner.clean(&ids, true).is_err());
        scanner.scan(|_| Ok(())).unwrap();
        assert!(scanner.clean(&ids, false).is_err());
        assert!(scanner.clean(&[], true).is_err());
        assert!(
            scanner
                .clean(&[ids[0].clone(), "../other".into()], true)
                .is_err()
        );
        assert!(
            scanner
                .clean(&[ids[0].clone(), ids[0].clone()], true)
                .is_err()
        );
        assert!(cache.join("data").exists());
        assert!(scanner.clean(&ids, true).unwrap().failures.is_empty());
        assert!(cache.exists());
        assert!(!cache.join("data").exists());
        assert!(scanner.clean(&ids, true).is_err());
    }

    #[test]
    fn streaming_reports_pending_and_completed_items_and_revokes_interrupted_scans() {
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("Library/Caches/Google");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("data"), [1; 4096]).unwrap();
        let mut scanner = Scanner::new(home.path().to_owned());
        let mut events = Vec::new();
        let snapshot = scanner
            .scan(|event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
        let ScanProgress::Started { items } = &events[0] else {
            panic!("expected placeholders before IO")
        };
        assert_eq!(items.len(), snapshot.items.len());
        assert!(
            items
                .iter()
                .all(|item| item.status == "待扫描" && !item.selectable)
        );
        assert!(matches!(&events[1], ScanProgress::Volume { .. }));
        let (pairs, remainder) = events[2..].as_chunks::<2>();
        assert!(remainder.is_empty());
        assert_eq!(pairs.len(), snapshot.items.len());
        for (index, pair) in pairs.iter().enumerate() {
            let ScanProgress::Scanning {
                id,
                completed,
                total,
            } = &pair[0]
            else {
                panic!("missing start")
            };
            assert_eq!(*completed, index);
            assert_eq!(*total, snapshot.items.len());
            let ScanProgress::Item {
                item, completed, ..
            } = &pair[1]
            else {
                panic!("missing result")
            };
            assert_eq!(&item.id, id);
            assert_eq!(*completed, index + 1);
        }
        assert!(scanner.eligible.contains("google-cache"));
        assert!(
            scanner
                .scan(|event| match event {
                    ScanProgress::Item { item, .. } if item.id == "google-cache" =>
                        Err("channel closed".into()),
                    _ => Ok(()),
                })
                .is_err()
        );
        assert!(scanner.clean(&["google-cache".into()], true).is_err());
        assert!(cache.join("data").exists());
    }

    #[test]
    fn inaccessible_and_busy_items_are_not_selectable() {
        let spec = &default_candidates(std::path::Path::new("/tmp/home"))[0];
        let mut result = ScanResult {
            reclaimable_bytes: 4096,
            ..Default::default()
        };
        assert!(selectable(spec, &result));
        result.open_handles = true;
        assert!(!selectable(spec, &result));
        result.open_handles = false;
        result.errors.push("permission denied".into());
        assert!(!selectable(spec, &result));
    }
}
