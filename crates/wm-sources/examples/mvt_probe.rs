//! geozero MVT API・レイヤ名・投影整合の確認用 probe（一時的・検証専用）。
//! 実行: `cargo run -p wm-sources --example mvt_probe -- <file.pbf> <z> <tx> <ty>`

use geozero::mvt::{Message, Tile};
use wm_core::geo::tile_frac_to_lonlat;

fn zigzag(n: u32) -> i32 {
    ((n >> 1) as i32) ^ (-((n & 1) as i32))
}

/// MVT ジオメトリのコマンド列 → タイルローカル整数座標のパス群。
fn decode_geom(geom: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let mut paths = Vec::new();
    let mut cur: Vec<(i32, i32)> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    while i < geom.len() {
        let cmd = geom[i] & 0x7;
        let count = (geom[i] >> 3) as usize;
        i += 1;
        match cmd {
            1 => {
                // MoveTo
                for _ in 0..count {
                    if i + 1 >= geom.len() {
                        break;
                    }
                    x += zigzag(geom[i]);
                    y += zigzag(geom[i + 1]);
                    i += 2;
                    if !cur.is_empty() {
                        paths.push(std::mem::take(&mut cur));
                    }
                    cur.push((x, y));
                }
            }
            2 => {
                // LineTo
                for _ in 0..count {
                    if i + 1 >= geom.len() {
                        break;
                    }
                    x += zigzag(geom[i]);
                    y += zigzag(geom[i + 1]);
                    i += 2;
                    cur.push((x, y));
                }
            }
            7 => {
                if let Some(&f) = cur.first() {
                    cur.push(f);
                }
            }
            _ => break,
        }
    }
    if !cur.is_empty() {
        paths.push(cur);
    }
    paths
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let z: u8 = a[2].parse()?;
    let tx: f64 = a[3].parse()?;
    let ty: f64 = a[4].parse()?;
    let bytes = std::fs::read(path)?;
    let tile = Tile::decode(&bytes[..])?;

    for layer in &tile.layers {
        if !matches!(layer.name.as_str(), "waterarea" | "boundary" | "road" | "railway") {
            continue;
        }
        let extent = layer.extent.unwrap_or(4096) as f64;
        let (mut min_lat, mut min_lon) = (f64::MAX, f64::MAX);
        let (mut max_lat, mut max_lon) = (f64::MIN, f64::MIN);
        let mut npts = 0usize;
        for f in &layer.features {
            for path in decode_geom(&f.geometry) {
                for (lx, ly) in path {
                    let xf = tx + lx as f64 / extent;
                    let yf = ty + ly as f64 / extent;
                    let (lat, lon) = tile_frac_to_lonlat(z, xf, yf);
                    min_lat = min_lat.min(lat);
                    max_lat = max_lat.max(lat);
                    min_lon = min_lon.min(lon);
                    max_lon = max_lon.max(lon);
                    npts += 1;
                }
            }
        }
        println!(
            "{:<12} pts={:<7} lat[{:.4}..{:.4}] lon[{:.4}..{:.4}]",
            layer.name, npts, min_lat, max_lat, min_lon, max_lon
        );
    }
    // 期待: タイル(227,100)@z8 の地理範囲 lat[35.39..36.51] lon[139.22..140.63]
    let (nw_lat, nw_lon) = tile_frac_to_lonlat(z, tx, ty);
    let (se_lat, se_lon) = tile_frac_to_lonlat(z, tx + 1.0, ty + 1.0);
    println!("tile geo: lat[{se_lat:.4}..{nw_lat:.4}] lon[{nw_lon:.4}..{se_lon:.4}]");
    Ok(())
}
