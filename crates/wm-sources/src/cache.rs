//! タイルのメモリキャッシュ。上限付き LRU。HTTP 取得の前段。
//!
//! 目的：パン/ズームの往復やタイムライン再生のコマ往復で、同じ (z,x,y) や
//! 同じ basetime/validtime のタイルを何度も取りに行く無駄を消す。`wm-core` は
//! 触らない（移植性の鉄則）。キャッシュはこの std 層だけの最適化。
//!
//! 種別ごとの寿命の違いは**インスタンスを分ける**ことで表現する（このモジュールは
//! 寿命を知らない）:
//! - 地図ベクトルタイル(bvmap .pbf)：内容がほぼ不変 → セッション中ずっと有効。
//! - 雨雲タイル(JMA PNG)：5分更新だが URL に basetime/validtime を含むので、
//!   同一フレームの再取得だけ防げばよい（古いフレームを掴むことはない）。
//!   **ディスクには永続化しない**（メモリのみ。古い雨雲を掴むリスク回避）。

use bytes::Bytes;
use directories::ProjectDirs;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// 地図タイルのディスクキャッシュ有効期限。experimental_bvmap はほぼ不変だが、
/// 実験提供でまれに更新されうるので 7 日で期限切れにする。
const DISK_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 複数の取得 task から共有するためのハンドル。
///
/// ロックはタイル 1 枚の get/put の間だけ保持する（HTTP の await は跨がない）ので
/// `std::sync::Mutex` で十分・短時間。ロック汚染時はキャッシュを諦めて素通しする。
pub type SharedCache = Arc<Mutex<TileCache>>;

/// 容量付き LRU タイルキャッシュ。
///
/// キーは**完全な URL**（z/x/y や basetime/validtime を含むので衝突しない）。
/// 値は `bytes::Bytes`（Arc 参照カウントで clone が安価）。
pub struct TileCache {
    map: HashMap<String, Bytes>,
    /// アクセス順（先頭=最古, 末尾=最新）。LRU 退避に使う。
    order: VecDeque<String>,
    capacity: usize,
    /// 命中/不命中カウンタ（キャッシュ効果の確認用。挙動には影響しない）。
    hits: u64,
    misses: u64,
}

impl TileCache {
    /// 最大 `capacity` エントリのキャッシュを作る。
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
        }
    }

    /// 共有ハンドル（`Arc<Mutex<_>>`）を作る。
    pub fn shared(capacity: usize) -> SharedCache {
        Arc::new(Mutex::new(Self::new(capacity)))
    }

    /// 取得（ヒット時は最新アクセス扱いに更新）。`Bytes` は安価 clone で返す。
    pub fn get(&mut self, url: &str) -> Option<Bytes> {
        let v = self.map.get(url)?.clone();
        self.touch(url);
        Some(v)
    }

    /// 格納。既存キーは値を更新。容量超過時は最古（LRU）を1件捨てる。
    pub fn put(&mut self, url: String, bytes: Bytes) {
        if self.map.contains_key(&url) {
            self.map.insert(url.clone(), bytes);
            self.touch(&url);
            return;
        }
        self.map.insert(url.clone(), bytes);
        self.order.push_back(url);
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// エントリ数（テスト・デバッグ用）。
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 命中を記録（`fetch_cached` が呼ぶ）。
    pub fn record_hit(&mut self) {
        self.hits += 1;
    }

    /// 不命中を記録（`fetch_cached` が呼ぶ）。
    pub fn record_miss(&mut self) {
        self.misses += 1;
    }

    /// (命中数, 不命中数)。キャッシュ効果の確認用。
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// `url` を最新アクセス位置（末尾）へ移す。
    fn touch(&mut self, url: &str) {
        if let Some(pos) = self.order.iter().position(|u| u == url) {
            if let Some(k) = self.order.remove(pos) {
                self.order.push_back(k);
            }
        }
    }
}

/// キャッシュ優先でタイルを取得する。ヒットならネットワークに行かない。
///
/// 挙動は「キャッシュ素通し版の HTTP 取得」と等価に保つ:
/// - 送信失敗・ボディ取得失敗 → `None`（呼び出し側は従来どおり当該タイルをスキップ）。
/// - 取得成功 → `Some(bytes)` を返す（デコード失敗時の従来挙動は呼び出し側のまま）。
/// - **キャッシュに入れるのは 2xx かつ非空のときだけ**。404/エラーボディはキャッシュ
///   しない（次回再取得できる＝古い欠損を固定しない。デコード失敗→スキップも従来どおり）。
pub async fn fetch_cached(
    client: &reqwest::Client,
    cache: &SharedCache,
    url: &str,
) -> Option<Bytes> {
    // 1. キャッシュ参照（ロックは get の間だけ。汚染時は素通し）。
    if let Ok(mut c) = cache.lock() {
        if let Some(b) = c.get(url) {
            c.record_hit();
            return Some(b);
        }
        c.record_miss(); // ここから HTTP に行く＝不命中
    }
    // 2. HTTP 取得。
    let resp = client.get(url).send().await.ok()?;
    let ok = resp.status().is_success();
    let bytes = resp.bytes().await.ok()?;
    // 3. 成功＆非空のみキャッシュへ。
    if ok && !bytes.is_empty() {
        if let Ok(mut c) = cache.lock() {
            c.put(url.to_string(), bytes.clone());
        }
    }
    Some(bytes)
}

/// `fetch_cached` のディスクキャッシュ付き版（**地図タイル専用**）。
///
/// メモリ miss → ディスク（TTL 内なら採用しメモリへ昇格）→ HTTP の3段。
/// 取得成功（2xx かつ非空）時はメモリとディスクの両方へ書く。
/// **雨雲タイルには使わない**：URL に basetime/validtime が入るため、
/// ディスクへ残すと古いフレームを掴む恐れがある（雨雲はメモリのみの `fetch_cached`）。
pub async fn fetch_cached_with_disk(
    client: &reqwest::Client,
    cache: &SharedCache,
    url: &str,
) -> Option<Bytes> {
    // 1. メモリ参照（ロックは get の間だけ。汚染時は素通し）。
    if let Ok(mut c) = cache.lock() {
        if let Some(b) = c.get(url) {
            c.record_hit();
            return Some(b);
        }
        c.record_miss();
    }

    // 2. ディスク参照（TTL 内なら採用し、メモリへ昇格）。
    let disk_path = disk_cache_path(url);
    if let Some(path) = disk_path.as_ref() {
        if let Some(b) = read_disk_if_fresh(path).await {
            if let Ok(mut c) = cache.lock() {
                c.put(url.to_string(), b.clone());
            }
            return Some(b);
        }
    }

    // 3. HTTP 取得。成功＆非空のみメモリ＋ディスクへ書く（欠損は固定しない）。
    let resp = client.get(url).send().await.ok()?;
    let ok = resp.status().is_success();
    let bytes = resp.bytes().await.ok()?;
    if ok && !bytes.is_empty() {
        if let Ok(mut c) = cache.lock() {
            c.put(url.to_string(), bytes.clone());
        }
        if let Some(path) = disk_path.as_ref() {
            write_disk(path, &bytes).await; // ベストエフォート（失敗は無視）
        }
    }
    Some(bytes)
}

/// タイル URL からディスクキャッシュのパスを導く。
///
/// `~/.cache/amescii/tiles/{z}/{x}/{y}.pbf`。URL 末尾3セグメント（z/x/y.ext）を使う。
/// ProjectDirs が引けない・想定外セグメント（非英数・`..` 等）の場合は `None`
/// を返し、ディスク層をスキップする（パストラバーサル防止）。
fn disk_cache_path(url: &str) -> Option<PathBuf> {
    let root = ProjectDirs::from("", "", "amescii")?.cache_dir().join("tiles");
    // rsplit なので取り出し順は [y.ext, x, z]。
    let tail: Vec<&str> = url.rsplit('/').take(3).collect();
    if tail.len() < 3 {
        return None;
    }
    let (y_ext, x, z) = (tail[0], tail[1], tail[2]);
    if !safe_seg(z) || !safe_seg(x) || !safe_seg(y_ext) {
        return None;
    }
    Some(root.join(z).join(x).join(y_ext))
}

/// パスセグメントとして安全か（英数と `.` のみ、`..` を含まない、空でない）。
fn safe_seg(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("..")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
}

/// ディスクキャッシュを TTL 内なら読む。無効・期限切れ・失敗は `None`。
async fn read_disk_if_fresh(path: &Path) -> Option<Bytes> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let modified = meta.modified().ok()?;
    // 経過時間が TTL 超（または mtime が未来）なら期限切れ扱い。
    match SystemTime::now().duration_since(modified) {
        Ok(age) if age <= DISK_TTL => {}
        _ => return None,
    }
    let data = tokio::fs::read(path).await.ok()?;
    if data.is_empty() {
        None
    } else {
        Some(Bytes::from(data))
    }
}

/// ディスクキャッシュへ書く（ベストエフォート。親ディレクトリを作成）。
async fn write_disk(path: &Path, bytes: &Bytes) {
    if let Some(dir) = path.parent() {
        if tokio::fs::create_dir_all(dir).await.is_err() {
            return;
        }
    }
    let _ = tokio::fs::write(path, bytes).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Bytes {
        Bytes::from(s.to_string())
    }

    #[test]
    fn get_put_hit_miss() {
        let mut c = TileCache::new(4);
        assert!(c.get("a").is_none());
        c.put("a".into(), b("AA"));
        assert_eq!(c.get("a").unwrap(), b("AA"));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn lru_evicts_oldest() {
        let mut c = TileCache::new(2);
        c.put("a".into(), b("A"));
        c.put("b".into(), b("B"));
        // a を触って最新化 → 次の挿入で b が落ちる。
        assert!(c.get("a").is_some());
        c.put("c".into(), b("C"));
        assert_eq!(c.len(), 2);
        assert!(c.get("a").is_some(), "最近使った a は残る");
        assert!(c.get("c").is_some(), "新しい c は残る");
        assert!(c.get("b").is_none(), "最古の b が退避される");
    }

    #[test]
    fn put_existing_updates_without_growth() {
        let mut c = TileCache::new(2);
        c.put("a".into(), b("A1"));
        c.put("a".into(), b("A2"));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("a").unwrap(), b("A2"));
    }

    #[test]
    fn capacity_is_respected() {
        let mut c = TileCache::new(3);
        for i in 0..10 {
            c.put(format!("k{i}"), b("x"));
        }
        assert_eq!(c.len(), 3);
        // 直近3件だけ残る。
        assert!(c.get("k9").is_some());
        assert!(c.get("k8").is_some());
        assert!(c.get("k7").is_some());
        assert!(c.get("k6").is_none());
    }
}
