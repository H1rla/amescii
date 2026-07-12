# 集約アルゴリズム詳細 (AGGREGATION.md)

複数の気象APIから得た同一指標の観測値を、単純平均ではなく**信頼度つき加重平均**で統合する。「サイトによって予報が違う」問題に対し、乖離を定量化（CV）し、明らかな外れ値を自動除外する。

すべて `wm-core::agg` 内の純粋関数。`no_std` + `libm`。

---

## 1. 入力と出力

入力: 1指標について複数の `Measurement`
```rust
Measurement { source: SourceId, value: f32, observed_at: u64 }
```

出力:
```rust
Aggregated { value, cv, confidence, n_used, n_excluded }
```

`aggregate()` には現在時刻 `now: u64` を**引数で**渡す（wm-coreは時刻を取得しない）。

```rust
pub fn aggregate(
    measurements: &[Measurement],
    now: u64,
    params: &AggParams,
) -> Aggregated;
```

---

## 2. アルゴリズム全体

```
入力: measurements[], now, params
─────────────────────────────────────────────
Step 1. 各 measurement に合成重みを計算
          w_i = w_static(source_i) × w_fresh(now - observed_at_i)

Step 2. 外れ値検出（MAD ベース修正 z-score）
          median = 値の中央値
          MAD    = median(|v_i - median|)
          M_i    = 0.6745 · |v_i - median| / MAD
          M_i > params.z_thresh のソースを除外マーク
          ・MAD ≈ 0（過半数が同値）の場合は平均絶対偏差 MeanAD に
            フォールバックし、係数 1/1.2533 を使う
          ・n < 3 （2ソース以下）は判定不能として除外しない
          ・ばらつき 0（全値同一）は除外しない

Step 3. 統計（生き残りのみ）
          μ = Σ(w_i · v_i) / Σ(w_i)
          σ = sqrt( Σ(w_i · (v_i - μ)²) / Σ(w_i) )
          value = μ

Step 4. 変動係数
          cv = σ / |μ|         (μ≈0 なら cv=0)

Step 5. 信頼度スコア
          confidence = f(cv, n_used, n_excluded)   （§5参照）
─────────────────────────────────────────────
出力: Aggregated { value=μ, cv, confidence, n_used, n_excluded }
```

> **なぜ MAD 修正 z-score か（重要）**：当初は「全ソースの加重平均・加重σ」での
> 通常 z-score（`z_i = |v_i-μ1|/σ1 > z_thresh`）を想定していたが、これは
> 外れ値自身がσを膨らませて自分を隠す **masking** に弱い。特に n=3 の小標本では、
> 明白な外れ値（例: 23.1 / 23.4 に対する 28.0）でも z が 1.5 程度にしかならず、
> どんな実用的閾値でも検出できない。中央値と MAD は外れ値の影響を受けにくいため、
> 28.0 → 修正 z ≈ 10.3、一致クラスタ（23.1/23.4/23.5）→ ≤ 2.0 と明確に分離できる。
> Iglewicz & Hoaglin (1993) の手法。実装は `wm-core/src/agg/outlier.rs`。

---

## 3. 静的重み w_static

各APIの日本国内における信頼度を反映した固定値。

| ソース | 重み | 根拠 |
|---|---|---|
| JMA（気象庁） | 1.0 | 日本の公式予報。国内基準。 |
| Open-Meteo | 0.9 | JMA seamless モデルだが別実装・別後処理。 |
| OpenWeatherMap | 0.8 | 欧州系グローバルモデル。国内では相対的に粗い。 |

```rust
fn w_static(s: SourceId) -> f32 {
    match s {
        SourceId::Jma => 1.0,
        SourceId::OpenMeteo => 0.9,
        SourceId::OpenWeatherMap => 0.8,
    }
}
```

> これらは `AggParams` で上書き可能にしておく（ユーザーが調整できる）。

---

## 4. 新鮮度重み w_fresh

観測からの経過時間で指数減衰。古いデータの寄与を下げる。

```
age = now - observed_at        （秒、負なら0にクランプ）
w_fresh = exp( -age / τ )
```

`τ`（時定数）は指標ごとに変える:
- 雨量・降水: τ = 1800秒（30分）。実況性が重要。
- 気温・湿度・風: τ = 5400秒（90分）。比較的ゆっくり変化。

```rust
fn w_fresh(age_secs: u64, tau_secs: f32) -> f32 {
    let age = age_secs as f32;
    libm::expf(-age / tau_secs)
}
```

> `exp` は `no_std` で使えないため `libm::expf`。

---

## 5. 信頼度スコア confidence

0.0〜1.0。3つの要素の積。

```
confidence = agreement × coverage × penalty

agreement = clamp(1 - cv / cv_max, 0, 1)
   ・ソース間が一致(cv小)なら1に近づく
   ・cv_max（例:0.20）で0になる

coverage = n_used / n_total_expected
   ・期待ソース数(3)のうち何個生きたか
   ・例: 2/3 = 0.67

penalty = 1 - 0.15 × n_excluded
   ・外れ値が出たら少し信頼度を下げる
```

実装:
```rust
fn confidence(cv: f32, n_used: u8, n_excluded: u8, p: &AggParams) -> f32 {
    let agreement = (1.0 - cv / p.cv_max).clamp(0.0, 1.0);
    let coverage  = (n_used as f32) / (p.n_expected as f32);
    let penalty   = 1.0 - 0.15 * (n_excluded as f32);
    (agreement * coverage * penalty).clamp(0.0, 1.0)
}
```

---

## 6. パラメータ構造体

```rust
pub struct AggParams {
    pub w_jma: f32,        // = 1.0
    pub w_open_meteo: f32, // = 0.9
    pub w_owm: f32,        // = 0.8
    pub tau_secs: f32,     // 指標により 1800 or 5400
    pub z_thresh: f32,     // = 3.5（MAD 修正 z-score の閾値, Iglewicz-Hoaglin）
    pub cv_max: f32,       // = 0.20
    pub n_expected: u8,    // = 3
}

impl AggParams {
    pub const fn for_precip() -> Self { /* tau=1800 */ }
    pub const fn for_slow() -> Self   { /* tau=5400 */ }
}
```

---

## 7. 風向の特殊処理

風向（方位角）は循環量なので算術平均できない（350°と10°の平均は0°であって180°ではない）。ベクトル平均する。

```
各ソースの風向 θ_i を単位ベクトルに分解:
   x_i = cos(θ_i),  y_i = sin(θ_i)
重み付き平均:
   x̄ = Σ w_i x_i / Σ w_i,   ȳ = Σ w_i y_i / Σ w_i
合成:
   θ_avg = atan2(ȳ, x̄)   （0..360に正規化）
   集中度 R = sqrt(x̄² + ȳ²)   （1なら完全一致、0ならバラバラ）
```

風向のCVの代わりに `1 - R` を乖離度として使う。`libm::cosf/sinf/atan2f` を使用。

---

## 8. 天気状態（condition）の多数決

`WeatherCode`（晴れ/曇り/雨/雪…）は数値ではないので加重多数決。

```
各 WeatherCode に静的重みを投票として加算し、最大票のコードを採用。
同票の場合は JMA を優先（最高重みソース）。
```

---

## 9. テストベクトル例

```rust
#[test]
fn excludes_outlier() {
    // JMA=23.1, OpenMeteo=23.4, OWM=28.0 (外れ値), 全て新鮮
    let now = 1000;
    let m = [
        Measurement { source: Jma,        value: 23.1, observed_at: 1000 },
        Measurement { source: OpenMeteo,  value: 23.4, observed_at: 1000 },
        Measurement { source: OpenWeatherMap, value: 28.0, observed_at: 1000 },
    ];
    let r = aggregate(&m, now, &AggParams::for_slow());
    assert_eq!(r.n_excluded, 1);          // 28.0 が除外される
    assert!((r.value - 23.2).abs() < 0.3); // 23.1と23.4の加重平均付近
    assert!(r.confidence > 0.5);
}

#[test]
fn all_agree_high_confidence() {
    // 3ソースほぼ一致 → CV小, confidence高
}

#[test]
fn stale_data_downweighted() {
    // 1つだけ2時間前 → 寄与が小さくなる
}

#[test]
fn wind_direction_wraps() {
    // 350° と 10° の平均が 0° 付近になる
}
```
