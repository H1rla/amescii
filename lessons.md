# lessons.md — weathermap

## 2026-06-28 — 初回コンパイル通し（骨格→緑）

### 外れ値検出：設計の z-score は自テストを通せない（masking）
**What happened**: `agg::tests::excludes_outlier`（{23.1, 23.4, 28.0} で 28.0 を除外）が失敗。
**Why**: 設計書 §2 の「全ソースの加重平均・加重σでの z-score（z>2.0 で除外）」は、
外れ値自身がσを膨らませて自分を隠す **masking** に弱い。n=3 の小標本では 28.0 の
z が約 1.54 にしかならず、実用的などの閾値でも検出不能（閾値を下げると一致クラスタ
誤除外と紙一重で脆い）。
**Rule going forward**: 小標本の外れ値検出は中央値・MAD ベースの修正 z-score
（Iglewicz-Hoaglin: `M = 0.6745·|x-median|/MAD`、MAD≈0 なら MeanAD フォールバック、
n<3 は判定しない）を使う。閾値は 3.5。実装 `wm-core/src/agg/outlier.rs`、
ドキュメントは AGGREGATION.md §2 を更新済み。

### no_std では f64 の std メソッドが使えない
**What happened**: `cargo build ... --features embedded --target riscv32imc-...` で
`f64::ln / floor / to_radians / to_degrees` が "no method found"。std テストビルドでは
std が供給するため気づけない。
**Why**: これらは std がプリミティブに生やすメソッド。no_std には無い。
**Rule going forward**: wm-core の数学は必ず libm 経由（`libm::log/floor`）、度⇄ラジアンは
手計算（`deg*PI/180`）。**移植性は std テストでは保証されない。RISC-V ビルドが唯一のガード。**
変更後は必ず `cargo build -p wm-core --no-default-features --features embedded
--target riscv32imc-unknown-none-elf` を回す。

### 時刻は引数で渡す（新鮮度減衰の落とし穴）
**What happened**: `aggregate_points` が内部で `now_unix()`（実時刻）を呼んでいたため、
`observed_at: 1000`（固定）のテストデータで新鮮度重み `exp(-age/τ)` が age≈17億秒 → 0 に
潰れ、n_used=0 で失敗。
**Why**: wm-core の鉄則「時刻は外部から u64 で渡す」が std 層 `aggregate_points` で
破られていた。
**Rule going forward**: 時刻に依存する集約関数は `now: u64` を引数で受ける。HTTP 取得の
入口（`fetch_and_aggregate`）でのみ `now_unix()` を取り、下流へは値で渡す。

### JMA forecast JSON は実構造とマッチ済み（位置非依存スキャンが奏功）
**What happened**: タスクで最大リスクとされた JMA JSON 構造は、実データ（130000.json）で
`weatherCodes` が block[0].timeSeries[0]、`temps` が block[0].timeSeries[2] にあり、
現コードの全 timeSeries 走査設計で正しく拾えた。serde が余分キーを無視するため修正不要。
**Rule going forward**: 抽出を純粋関数 `extract_temp_and_code` に分離し、実構造を縮約した
JSON で回帰テスト済み（`providers/jma.rs`）。構造変化はこのテストで検知する。

### ratatui 0.28 / image 0.25 は記述どおりで API 差なし
`buf[(x,y)]`（Index 実装）と `Frame::area()`、`image::load_from_memory` + `get_pixel().0`
（`[u8;4]`）はそのまま通った。タスクで警戒された箇所だが修正不要だった。

## 2026-06-28 — 雨雲タイムライン追補（地図層は実装済み・タイムライン未実装だった）

### 引き継ぎ状態：radar.rs が `TARGET_TIMES` 未定義でコンパイル不能
**What happened**: 機能1（地図ベース層 lines.rs/basemap.rs/map.rs）は完成・緑だったが、
機能2（タイムライン）は手付かずで、`radar.rs` が定数 `TARGET_TIMES`（定義は N1/N2）を
参照して `cargo check` が即落ちていた。`NOWCAST_FRAMES`/`FORECAST_FRAMES` も未使用。
**Rule going forward**: 追補タスクは「どこまで出来てどこが壊れているか」を最初に
`cargo check` 全クレートで確定させてから着手する。ファイルが在る≠実装済み。

### heapless インラインの巨大型を async で値渡しすると tokio worker がスタックオーバーフロー
**What happened**: 擬似端末で起動した瞬間に `tokio-rt-worker has overflowed its stack`。
原因は `Grid`（`heapless::Vec<f32, 65536>` を**インライン保持＝常に256KB**、width に依らず）を
`fetch_frame_grid` が値で返し・await を跨いで保持していたこと。debug ビルドは RVO が無く、
`new_zeroed→関数戻り→Box::new` で 256KB が複数同時にスタックへ載り、既定 2MB worker を溢れさせた。
**Why**: `heapless::Vec<T, N>` の `N` は容量＝インラインサイズ。実要素数と無関係に常時最大サイズ。
quantize/rasterize_lines の戻り値 `Vec<_, 12000>`（≈192KB）も同様の地雷。
**Rule going forward**: wm-core が返す heapless 巨大型は **生成直後に `Box` 化し、await/関数境界を
ポインタで渡す**（`fetch_frame_grid -> Result<Box<Grid>>`）。加えて保険として tokio ランタイムを
手組みし `thread_stack_size(16MB)`。wm-core の型（移植性に関わる）は変えない方針なのでこの2段構え。
**検証**: `#[tokio::main]` だと PTY 無し環境では `enable_raw_mode` が ENXIO(os error 6) で落ちるのは
正常（制御端末が無いだけ）。TUI の実描画確認は `pty.fork()` + `TIOCSWINSZ` で窓サイズを与える。

### 時刻は wm-sources で u64 化（validtime "YYYYMMDDHHMMSS"(UTC) → unix）
**What happened**: `RadarFrame.valid_unix` 用に validtime を unix へ。chrono を wm-sources に
足さず、Howard Hinnant の `days_from_civil` を自前実装（純粋・テスト可）。表示の HH:MM 整形だけ
wm-tui 側で chrono(clock)。鉄則「時刻は外部から u64」を層境界で守る。
**Rule going forward**: 暦→unix の既知アンカー（epoch=0, 2000-01-01=946684800）を必ずテストに置く。
手計算の期待値は信用しない（実際 4 日ズレを埋め込んでテストが弾いた。正解は `date -u -d ... +%s`）。

## 2026-06-28 — 都市/地名ラベル + 雨雲トグル追補

### experimental_bvmap の `label` レイヤにローマ字名は無い（設計書の前提が実データと相違）
**What happened**: 設計書は「label レイヤの `name`（ローマ字, 例 "Kanazawa"）を拡大時に英語表示」と
規定していたが、実タイル（z=10/12/14/16, 東京駅）をデコードすると **`name` キーが存在しない**。
実 keys は `annoChar`(表示注記=日本語)/`knj`(漢字)/`kana`(かな)/`ftCode`、すべて非ASCII。
`ftCode` は注記が一律 100 で種別フィルタに使えない。z=14 以上では `annoChar` すら消え `knj`/`kana` のみ、
`kana` も欠損地物あり。設計書の golden example `{"name":"Kanazawa", "knj":"金沢"}` は現行データと不一致。
**Why**: 提供実験データの仕様変更か、設計書が別ソース（optimal_bvmap 等）を参照していた可能性。
**Rule going forward**: 外部ベクトルタイルの属性は**実タイルを1枚デコードして keys/値を目視**してから
デコード処理を確定する（設計書の属性記述を信用しない）。プロパティは MVT の `tags`（[key_idx,val_idx,...]
フラット列）を `layer.keys`/`layer.values` で索く。確認用に `geozero::mvt::tile::{Layer,Feature,Value}` を直接使う
使い捨て example が有効。**判明後はユーザーに方針を確認**（英語一貫 vs 日本語表示 vs label断念）。
今回は「内蔵英語都市テーブルのみ・label 断念」を選択 → wm-sources の label デコードは実装せず除去（dead code 禁止）。

### ラベル配置は lines/radar と同じ投影を共有（`render::lines::project_norm` を pub(crate) 流用）
**What happened**: `places::layout_city_labels` のセル投影は `crate::render::lines::project_norm`
（BBox 正規化, zoom 相殺）をそのまま使い、雨雲ドットの quantize セルと一致させた。回帰テスト
`projection_matches_rain`：同一 lat/lon で `marker_col/row == quantize cell` を assert。
**Why**: 別式で投影するとラベルが地図・雨雲とズレる。floor の二重適用 `floor(floor(2x)/2)=floor(x)` で
文字セルとドットセルが厳密一致する。
**Rule going forward**: 画面投影が要るロジックは新式を起こさず既存 `project_norm` を共有する。

### TUI の画面状態検証は pyte（VT エミュレータ）で確定画面を読む
**What happened**: ratatui は差分のみ出力するため、PTY 生バイトのスライス集計は画面状態の真値に
ならない（変化セルしか出ない）。`pyte.Screen` に全ストリームを feed し `screen.display` を読むと
確定画面の文字が得られ、都市名・`·`マーカー数・タイムライン有無・braille 数を正確に測れた。
**Rule going forward**: TUI の見た目検証は pyte（scratchpad の venv）＋ `TIOCSWINSZ` で窓サイズ付与。
`show_radar` のように複数描画を1つの bool でゲートする場合、観測しやすい方（timeline の出没）が
他方（雨雲）のゲート証拠になる。

## 2026-06-28 — 詳細ズーム(z16) + 日本語地名 + 雨雲オーバーズーム追補

### 地図ズームと雨雲ズームの分離：雨雲は取得ズームだけクランプすれば bbox 正規化で自動整合
**What happened**: app zoom 上限を 10→16 に上げ、雨雲は z10 上限のオーバーズーム表示。
`radar.rs::fetch_frame_grid` は単一 `zoom` 引数で「タイル取得（z）」と「bbox正規化（px_min/px_max を z で算出）」
を一貫計算し、グリッドが覆う地理範囲は引数 `bbox`（現在の狭い bbox）のまま。よって
**`let zoom = zoom.min(RADAR_MAX_ZOOM)` を関数先頭で1行入れるだけ**で「z10 でタイル→緯度経度、
現在 bbox で緯度経度→セル」が成立し、雨雲が地図とズレない（doc が警戒した投影取り違えは起きない）。
**Why**: 正規化 `(gpx-px_min)/span` は gpx も px_min も同じ z10 ピクセルなので、bbox 内の相対位置を返す。
地図線・ラベルの `project_norm`（bbox 正規化）と同じ「bbox 内相対位置」になり一致する。
**Rule going forward**: オーバーズームは「取得ズームをクランプ＋表示 bbox は据え置き」。投影は
取得ズーム一貫で計算すれば表示ズームと取り違えない。z12 で `status=雨雲更新済み（Nコマ）` を確認（404 回避の証拠）。

### label レイヤのテキストは annoChar 優先・knj フォールバック（前回の "name 無し" を踏まえ再調査）
**What happened**: 拡大時の日本語地名。前回 `name`(ローマ字) が無いと判明済みなので、今回は
コード前に実タイル z11/13/15 をデコードして充足率を目視：**`knj`(漢字)=100%充足**（85/85,30/30,36/36）、
`annoChar`=76/85→10/30→**0/36**（高ズームで消える）、`kana` も部分的。`ftCode` 全件あるが種別判別は不明。
→ テキストは `annoChar.or(knj)`。これで z11–16 全帯で日本語地名が必ず出る（"東京駅"/"丸の内二丁目" 等）。
件数は数十/タイルなので wm-tui 側で間引き（短い注記優先＋同一行重なり除外＋1画面 MAX_JA_LABELS=40）。
**Rule going forward**: ベクトルタイルの注記キーはズーム帯で充足率が変わる。最も充足率の高いキー（knj）を
フォールバックに据え、簡潔表示用（annoChar）を優先する二段構え。**コード前に充足率を目視**（前回の教訓）。

### 投影は wm-core の公開 `lonlat_to_cell` に一本化（英語都市・日本語地名・雨雲で共有）
**What happened**: 緯度経度→文字セルを `render::lines::lonlat_to_cell`（pub, project_norm ベース）に集約。
places.layout_city_labels もこれを使うよう refactor、wm-tui の日本語ラベルも同関数で投影。投影式が1つなので
ラベルと地図・雨雲が必ず同じセルに乗る。no_std 維持（wm-core 内・libm 経由）。
**Rule going forward**: 「データは外・描画(投影)は中」。画面投影が要るなら wm-core に公開関数を1つ置き全員that使う。

### 全角ラベルは ratatui の `Buffer::set_stringn` に任せる（unicode-width で幅算出）
**What happened**: 日本語（全角=2セル）の描画は手書きせず `set_stringn(x,y,s,max_w,style)` に委譲。
ratatui が unicode-width で幅2を処理し continuation セルを空にする。間引きの重なり判定用の表示幅だけ
`unicode_width::UnicodeWidthStr::width` で算出。黒背景＋白文字で地図・雨雲の上でも可読。
**Rule going forward**: 端末の多バイト・全角描画は自前でセル送りせず set_stringn に任せる。

### ◎ 中心マーカーはラベルより先に描く（"◎okyo" 修正）。代償は中心がラベルに覆われると ◎ 不可視
**What happened**: 前回「◎ がラベル先頭文字を潰す（◎okyo）」を、◎ をラベルより**先**に描くことで修正
（doc の選択肢1）。pyte で `Tokyo` が完全語として検出＝潰れ解消を確認、`◎[漢字]` パターンも 0。
ただし高ズームで中心セルが密な日本語ラベルに覆われると ◎ 自体が隠れる（中心＝画面中央で自明なので許容）。
**Rule going forward**: 重なる前景の優先順位は「先に描く＝下、後に描く＝上」。読ませたい方（地名）を後に。

## 2026-06-28 — 軽量化（Cargo プロファイル + タイルキャッシュ + デバウンス）

### リリースプロファイルで PC バイナリ 5.39MB→2.26MB（-60%）。`panic="abort"` は入れない
**What happened**: ルート Cargo.toml に `[profile.release] opt-level="z", lto=true, codegen-units=1, strip=true`。
コードは不変で **5,657,336B → 2,377,984B（-60%）**、stripped。ビルドは ~48s（lto+codegen-units=1）。
**Why panic=abort を避ける**: このアプリは panic hook で raw mode/代替画面を復帰させる。abort だと hook が
走らず端末が壊れた状態で残る経路ができる。サイズより端末復帰の安全を優先。
**衝突確認**: wm-esp32 は workspace members から外れ独自 `[profile.release]` を持つが、別ビルド扱いなので
ルートの release プロファイルと衝突しない（riscv embedded ビルドは緑のまま）。

### タイルキャッシュは std 層(wm-sources)だけ・URL キー・Bytes 値・命中カウンタで実証
**What happened**: `cache.rs`（`TileCache` 容量付き LRU + `fetch_cached` ヘルパ）。キーは完全 URL
（z/x/y や basetime/validtime を含むので衝突しない）、値は `bytes::Bytes`（Arc 参照カウントで clone 安価）。
`std::sync::Mutex` で包み、ロックは get/put の間だけ（HTTP の await は跨がない）。
- **地図(bvmap .pbf)と日本語ラベルは同一 URL を引く** → map_cache を共有すると相互ヒット（実証 hits=1/misses=1）。
- **雨雲(JMA PNG)はメモリのみ**＝ディスク永続化しない（URL に basetime/validtime を含むので同一フレーム再取得だけ防ぐ。
  古いフレームを掴むことはない）。
- 404/エラーボディはキャッシュしない（2xx かつ非空のみ put）＝欠損を固定せず再取得できる。デコード失敗→スキップの従来挙動も維持。
**検証**: 命中カウンタ（hits/misses）を TileCache に持たせ、使い捨て example で同一 URL×2 を実ネットワークで叩き
「2回目 misses=1/hits=1・bytes 同一」を確認 → example 削除（dead code 禁止）。
**Rule going forward**: 取得キャッシュは「キー=完全URL・値=Bytes・std Mutex 短時間ロック・成功時のみ put」。
寿命の違い（地図=不変/雨雲=フレーム単位）はインスタンス分離で表す。キャッシュ層は wm-core に持ち込まない。

### 連打デバウンスは「pending フラグ + 最終入力時刻」で既存ティックに乗せる（abort 不要）
**What happened**: パン/ズーム連打で中間状態のタイルを取りに行く無駄を、入力時に即取得せず
`pending_refetch=true; last_input=now` を立て、ループ末で `last_input.elapsed() >= 200ms` のとき1回だけ
`trigger_refetch`。既存の select! の sleep(TICK=100ms) が定期起床を担うので追加タイマー不要。zoom_in 等の状態
更新は handle_key で即時（見た目は即反応）、取得だけ遅延＝挙動は変わらず取得回数だけ減る。JoinHandle::abort は使わず単純化。
**検証**: pyte で `+`×8 を 30ms 間隔（200ms 窓内）に連打 → z16 到達＋地図/日本語ラベルがロード（最終状態で1回取得）。
**Rule going forward**: デバウンスは新タイマーを増やさず「フラグ＋最終時刻＋既存ティック」で十分。状態更新と取得を分け、
取得だけ遅延させれば見た目の即時性は保てる。
