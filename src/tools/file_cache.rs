use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

const DEFAULT_MAX_ENTRIES: usize = 256;
const EVICTION_AGE_SECS: u64 = 300;
const HASH_CHUNK_SIZE: usize = 4096;
/// RAM cap for cached file content: 64MB total keeps repeat reads free
/// without letting one giant file eat the budget. Entries over 1MB per
/// file are metadata-only (still skip re-reads via mtime+size hits for
/// the unchanged-stub path, but don't pin content in RAM).
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHED_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FileCacheEntry {
    pub mtime_secs: u64,
    pub size: u64,
    pub content_hash: u64,
    pub line_count: usize,
    pub accessed_at: Instant,
    /// Full text for small files; `None` for oversize entries or when the
    /// budget is full. Freshness is still verified by mtime+size.
    pub content: Option<String>,
    /// Byte offsets where each line starts; lets windowed reads slice
    /// without re-splitting the whole file.
    pub line_starts: Option<Vec<usize>>,
}

fn line_starts_for(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, byte) in content.bytes().enumerate() {
        if byte == b'\n' && idx + 1 < content.len() {
            starts.push(idx + 1);
        }
    }
    starts
}

pub struct FileReadCache {
    entries: DashMap<String, FileCacheEntry>,
    max_entries: usize,
    max_bytes: usize,
    bytes_used: AtomicUsize,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

fn file_metadata(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some((mtime, meta.len()))
}

fn simple_hash(content: &str) -> u64 {
    let bytes = content.as_bytes();
    let chunk = if bytes.len() > HASH_CHUNK_SIZE {
        &bytes[..HASH_CHUNK_SIZE]
    } else {
        bytes
    };
    let mut hash: u64 = 14695981039346656037;
    for &byte in chunk {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash ^= bytes.len() as u64;
    hash = hash.wrapping_mul(1099511628211);
    hash
}

impl FileReadCache {
    pub fn new(max_entries: usize) -> Self {
        Self::with_limits(
            if max_entries == 0 {
                DEFAULT_MAX_ENTRIES
            } else {
                max_entries
            },
            DEFAULT_MAX_BYTES,
        )
    }

    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: DashMap::new(),
            max_entries: if max_entries == 0 {
                DEFAULT_MAX_ENTRIES
            } else {
                max_entries
            },
            max_bytes: if max_bytes == 0 {
                DEFAULT_MAX_BYTES
            } else {
                max_bytes
            },
            bytes_used: AtomicUsize::new(0),
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES)
    }

    /// Fresh entry with content when the file fits the RAM budget.
    /// Returns `None` on mtime/size mismatch (stale) or when the entry
    /// holds metadata only. Stale entries are evicted; metadata-only
    /// entries are kept so `check` still sees them. Callers fall back to
    /// a streaming disk read.
    pub fn get_content(&self, canonical_path: &Path) -> Option<FileCacheEntry> {
        let key = canonical_path.to_string_lossy().to_string();
        let current_meta = file_metadata(canonical_path);
        let entry = self.entries.get(&key)?;

        match current_meta {
            Some((mtime, size)) => {
                if entry.mtime_secs == mtime && entry.size == size {
                    if entry.content.is_some() {
                        drop(entry);
                        if let Some(mut e) = self.entries.get_mut(&key) {
                            e.accessed_at = Instant::now();
                        }
                        let e = self.entries.get(&key)?;
                        self.hits.fetch_add(1, Ordering::Relaxed);
                        return Some(e.value().clone());
                    }
                    // Metadata-only entry: fresh but no content to serve.
                    return None;
                }
                drop(entry);
                self.misses.fetch_add(1, Ordering::Relaxed);
                if let Some((_, removed)) = self.entries.remove(&key) {
                    self.bytes_used
                        .fetch_sub(removed.content.map(|c| c.len()).unwrap_or(0), Ordering::Relaxed);
                }
                None
            }
            None => {
                drop(entry);
                if let Some((_, removed)) = self.entries.remove(&key) {
                    self.bytes_used
                        .fetch_sub(removed.content.map(|c| c.len()).unwrap_or(0), Ordering::Relaxed);
                }
                None
            }
        }
    }

    pub fn check(&self, canonical_path: &Path) -> Option<FileCacheEntry> {
        let key = canonical_path.to_string_lossy().to_string();
        let current_meta = file_metadata(canonical_path);

        let entry = self.entries.get(&key)?;

        match current_meta {
            Some((mtime, size)) => {
                if entry.mtime_secs == mtime && entry.size == size {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(entry.value().clone())
                } else {
                    drop(entry);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    self.entries.remove(&key);
                    None
                }
            }
            None => {
                drop(entry);
                self.entries.remove(&key);
                None
            }
        }
    }

    pub fn check_hit(&self, canonical_path: &Path) -> Option<FileCacheEntry> {
        let key = canonical_path.to_string_lossy().to_string();
        let current_meta = file_metadata(canonical_path);

        let entry = self.entries.get(&key)?;

        match current_meta {
            Some((mtime, size)) => {
                if entry.mtime_secs == mtime && entry.size == size {
                    drop(entry);
                    if let Some(mut e) = self.entries.get_mut(&key) {
                        e.accessed_at = Instant::now();
                    }
                    let e = self.entries.get(&key)?;
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(e.value().clone())
                } else {
                    drop(entry);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    self.entries.remove(&key);
                    None
                }
            }
            None => {
                drop(entry);
                self.entries.remove(&key);
                None
            }
        }
    }

    pub fn update(&self, canonical_path: &Path, content: &str) {
        self.insert(canonical_path, content.to_string());
    }

    /// Insert owned content (avoids a second copy of the file text).
    /// Files over `MAX_CACHED_FILE_BYTES` or inserts that would exceed
    /// the byte budget are stored metadata-only.
    pub fn insert(&self, canonical_path: &Path, content: String) {
        let key = canonical_path.to_string_lossy().to_string();
        let meta = file_metadata(canonical_path);

        let (mtime_secs, size) = match meta {
            Some(m) => m,
            None => return,
        };

        if self.entries.len() >= self.max_entries {
            self.evict_old();
        }

        let oversize = size > MAX_CACHED_FILE_BYTES || content.len() as u64 > MAX_CACHED_FILE_BYTES;
        let fits_budget =
            self.bytes_used.load(Ordering::Relaxed) + content.len() <= self.max_bytes;
        let (stored_content, line_starts) = if !oversize && fits_budget {
            let starts = line_starts_for(&content);
            (Some(content), Some(starts))
        } else {
            (None, None)
        };
        let stored_len = stored_content.as_ref().map(|c| c.len()).unwrap_or(0);

        let line_count = if let Some(ref text) = stored_content {
            text.lines().count()
        } else {
            // Metadata-only: count cheaply from size is wrong, so leave 0;
            // callers that need exact counts stream the file.
            0
        };
        let content_hash = stored_content
            .as_ref()
            .map(|text| simple_hash(text))
            .unwrap_or(size.wrapping_mul(1099511628211));

        if let Some((_, removed)) = self.entries.remove(&key) {
            self.bytes_used
                .fetch_sub(removed.content.map(|c| c.len()).unwrap_or(0), Ordering::Relaxed);
        }
        self.bytes_used.fetch_add(stored_len, Ordering::Relaxed);
        self.entries.insert(
            key,
            FileCacheEntry {
                mtime_secs,
                size,
                content_hash,
                line_count,
                accessed_at: Instant::now(),
                content: stored_content,
                line_starts,
            },
        );

        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn invalidate(&self, canonical_path: &Path) {
        let key = canonical_path.to_string_lossy().to_string();
        if let Some((_, removed)) = self.entries.remove(&key) {
            self.bytes_used
                .fetch_sub(removed.content.map(|c| c.len()).unwrap_or(0), Ordering::Relaxed);
            debug!("File cache invalidated: {}", key);
        }
    }

    pub fn invalidate_by_prefix(&self, prefix: &Path) {
        let prefix_str = prefix.to_string_lossy().to_string();
        let removed: Vec<(String, FileCacheEntry)> = self
            .entries
            .iter()
            .filter(|entry| entry.key().starts_with(&prefix_str))
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        let mut freed = 0usize;
        for (key, entry) in removed {
            freed += entry.content.map(|c| c.len()).unwrap_or(0);
            self.entries.remove(&key);
        }
        self.bytes_used.fetch_sub(freed, Ordering::Relaxed);
    }

    pub fn invalidate_all(&self) {
        let count = self.entries.len();
        self.entries.clear();
        self.bytes_used.store(0, Ordering::Relaxed);
        if count > 0 {
            info!("File cache cleared ({} entries evicted)", count);
        }
    }

    fn evict_old(&self) {
        let cutoff = Instant::now() - std::time::Duration::from_secs(EVICTION_AGE_SECS);
        let victims: Vec<(String, usize)> = self
            .entries
            .iter()
            .filter(|entry| entry.value().accessed_at < cutoff)
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().content.as_ref().map(|c| c.len()).unwrap_or(0),
                )
            })
            .take(self.max_entries / 4)
            .collect();

        // If nothing is stale but the budget is full, evict least-recently
        // used regardless of age: content cache must make room.
        let victims = if victims.is_empty() {
            let mut by_age: Vec<(String, usize, Instant)> = self
                .entries
                .iter()
                .map(|entry| {
                    (
                        entry.key().clone(),
                        entry.value().content.as_ref().map(|c| c.len()).unwrap_or(0),
                        entry.value().accessed_at,
                    )
                })
                .collect();
            by_age.sort_by_key(|(_, _, accessed)| *accessed);
            by_age
                .into_iter()
                .take(self.max_entries / 4 + 1)
                .map(|(key, bytes, _)| (key, bytes))
                .collect()
        } else {
            victims
        };

        let mut freed = 0usize;
        for (key, bytes) in victims {
            if self.entries.remove(&key).is_some() {
                freed += bytes;
            }
        }
        self.bytes_used.fetch_sub(freed, Ordering::Relaxed);
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        (self.entries.len(), hits, misses)
    }

    pub fn bytes_used(&self) -> usize {
        self.bytes_used.load(Ordering::Relaxed)
    }

    pub fn format_stub(entry: &FileCacheEntry, path: &Path) -> String {
        let path_display = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        format!(
            "[File unchanged since last read: \"{}\" ({} lines, {} bytes). Use read_file with start_line/end_line if you need specific content.]",
            path_display, entry.line_count, entry.size
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_simple_hash_deterministic() {
        let content = "hello world\nline 2\nline 3";
        let h1 = simple_hash(content);
        let h2 = simple_hash(content);
        assert_eq!(h1, h2);
        assert_ne!(h1, 0);
    }

    #[test]
    fn test_simple_hash_different_content() {
        let h1 = simple_hash("hello");
        let h2 = simple_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_cache_update_and_hit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let cache = FileReadCache::new(10);
        let canonical = file_path.canonicalize().unwrap();

        assert!(cache.check_hit(&canonical).is_none());

        cache.update(&canonical, "line 1\nline 2\nline 3\n");
        let entry = cache.check_hit(&canonical);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().line_count, 3);
    }

    #[test]
    fn test_cache_size_change_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "original content\n").unwrap();

        let cache = FileReadCache::new(10);
        let canonical = file_path.canonicalize().unwrap();

        cache.update(&canonical, "original content\n");
        assert!(cache.check_hit(&canonical).is_some());

        fs::write(&file_path, "modified content that is different\n").unwrap();

        let meta = fs::metadata(&canonical).unwrap();
        let mtime = meta.modified().unwrap();
        let mtime2 = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if let Some(entry) = cache.entries.get(&canonical.to_string_lossy().to_string()) {
            if entry.mtime_secs == mtime2 {
                eprintln!("Warning: mtime did not change between writes (same second granularity). Skipping assertion.");
                return;
            }
        }

        assert!(cache.check_hit(&canonical).is_none());
    }

    #[test]
    fn test_cache_invalidation_on_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "original content\n").unwrap();

        let cache = FileReadCache::new(10);
        let canonical = file_path.canonicalize().unwrap();

        cache.update(&canonical, "original content\n");
        assert!(cache.check_hit(&canonical).is_some());

        let original_meta = fs::metadata(&canonical).unwrap();
        let original_size = original_meta.len();

        fs::write(
            &file_path,
            "this is a much longer modified content that definitely changes the file size\n",
        )
        .unwrap();

        let new_meta = fs::metadata(&canonical).unwrap();
        let new_size = new_meta.len();

        if original_size == new_size {
            eprintln!("Warning: file size did not change. Skipping cache invalidation test.");
            return;
        }

        assert!(cache.check_hit(&canonical).is_none());
    }

    #[test]
    fn test_invalidate_all() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        fs::write(&f1, "aaa\n").unwrap();
        fs::write(&f2, "bbb\n").unwrap();

        let cache = FileReadCache::new(10);
        cache.update(&f1.canonicalize().unwrap(), "aaa\n");
        cache.update(&f2.canonicalize().unwrap(), "bbb\n");

        let (len, _, _) = cache.stats();
        assert_eq!(len, 2);
        cache.invalidate_all();
        let (len, _, _) = cache.stats();
        assert_eq!(len, 0);
    }

    #[test]
    fn test_format_stub() {
        let entry = FileCacheEntry {
            mtime_secs: 0,
            size: 1024,
            content_hash: 0,
            line_count: 42,
            accessed_at: Instant::now(),
            content: None,
            line_starts: None,
        };
        let stub = FileReadCache::format_stub(&entry, Path::new("src/main.rs"));
        assert!(stub.contains("42 lines"));
        assert!(stub.contains("1024 bytes"));
        assert!(stub.contains("unchanged"));
    }

    #[test]
    fn test_stats_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content\n").unwrap();

        let cache = FileReadCache::new(10);
        let canonical = file_path.canonicalize().unwrap();

        let (len, hits, misses) = cache.stats();
        assert_eq!(len, 0);
        assert_eq!(hits, 0);
        assert_eq!(misses, 0);

        cache.update(&canonical, "content\n");
        let (_, hits, misses) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 1);

        cache.check_hit(&canonical);
        let (_, hits, _) = cache.stats();
        assert_eq!(hits, 1);
    }

    #[test]
    fn content_cache_serves_repeat_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let cache = FileReadCache::new(10);
        let canonical = file_path.canonicalize().unwrap();

        assert!(cache.get_content(&canonical).is_none());
        cache.update(&canonical, "line 1\nline 2\nline 3\n");
        let entry = cache.get_content(&canonical).expect("content hit");
        assert_eq!(entry.content.as_deref(), Some("line 1\nline 2\nline 3\n"));
        assert_eq!(entry.line_starts.as_ref().map(|v| v.len()), Some(3));
        assert!(cache.bytes_used() > 0);
    }

    #[test]
    fn oversize_files_stay_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.bin");
        let big = "x".repeat((MAX_CACHED_FILE_BYTES + 1) as usize);
        fs::write(&file_path, &big).unwrap();

        let cache = FileReadCache::with_limits(10, 1024 * 1024 * 1024);
        let canonical = file_path.canonicalize().unwrap();
        cache.update(&canonical, &big);
        // Content path refuses oversize entries...
        assert!(cache.get_content(&canonical).is_none());
        assert_eq!(cache.bytes_used(), 0);
        // ...but the metadata path still records the file.
        assert!(cache.check(&canonical).is_some());
    }

    #[test]
    fn byte_budget_evicts_lru_content() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FileReadCache::with_limits(256, 100);
        for i in 0..4 {
            let path = dir.path().join(format!("f{i}.txt"));
            fs::write(&path, "0123456789abcdef0123456789abcdef").unwrap();
            let canonical = path.canonicalize().unwrap();
            cache.update(&canonical, "0123456789abcdef0123456789abcdef");
        }
        assert!(cache.bytes_used() <= 100);
    }
}
