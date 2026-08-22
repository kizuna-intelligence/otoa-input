//! VAD を実時間相当で回したときの CPU 使用を測る。GUI を出さずに確かめるため。
//! `cargo run --release -p otoa-input-vad --example cpu_probe`
fn main() -> anyhow::Result<()> {
    // sherpa-onnx が持つ ONNX Runtime を実行ファイルへ引き込むための参照。
    // crates/onnx のテストと同じ理由（OrtGetApiBase の解決）。
    let _link_sherpa = std::mem::size_of::<sherpa_onnx::OfflineRecognizerConfig>();
    let mut vad = otoa_input_vad::SileroVad::bundled()?;
    println!("スレッド数(起動直後): {}", threads());
    let hop = vec![0i16; 512];
    let mut out = Vec::new();
    let started = std::time::Instant::now();
    let cpu0 = cpu_seconds();
    let mut frames = 0u32;
    let mut lat: Vec<f64> = Vec::new();
    // 32ms ごとに 1 回。実時間 10 秒ぶん。
    while started.elapsed() < std::time::Duration::from_secs(4) {
        out.clear();
        let t = std::time::Instant::now();
        vad.push(&hop, &mut out)?;
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        frames += 1;
        std::thread::sleep(std::time::Duration::from_millis(32));
    }
    let wall = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu0;
    println!("スレッド数(実行中): {}", threads());
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| lat[((lat.len() as f64 - 1.0) * p) as usize];
    println!(
        "1フレームの処理時間 中央値 {:.3}ms / p95 {:.3}ms / 最大 {:.3}ms",
        pct(0.5),
        pct(0.95),
        pct(1.0)
    );
    println!(
        "フレーム {frames} / 実時間 {wall:.1}s / CPU {cpu:.2}s → {:.0}% (1コア=100%)",
        cpu / wall * 100.0
    );
    Ok(())
}

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0)
}

fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat
        .rsplit(')')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    let utime: f64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let stime: f64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (utime + stime) / 100.0
}
