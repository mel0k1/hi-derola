use hi_derola::lsp;

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join(format!("hiderola-lsp-smoke3-{}", std::process::id()));
    std::fs::create_dir_all(&dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"smoke\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    let p = dir.join("src/main.rs");
    std::fs::write(&p, "fn main() {\n    let _x: i32 = \"oops\";\n}\n").unwrap();
    let path = p.display().to_string();
    std::env::set_current_dir(&dir).unwrap();

    let t0 = std::time::Instant::now();
    let first = lsp::diagnose(&path).await;
    println!(
        "call 1 ({:.1}s): {:?}",
        t0.elapsed().as_secs_f32(),
        first.as_deref().map(|s| s.lines().count())
    );

    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    std::fs::write(&p, "fn main() {\n    let _x: i32 = \"oops\";\n    let _y: f64 = 1;\n}\n").unwrap();
    let t1 = std::time::Instant::now();
    match lsp::diagnose(&path).await {
        Some(out) => println!("OK call 2 ({:.1}s):\n{out}", t1.elapsed().as_secs_f32()),
        None => println!("NO DIAGNOSTICS call 2 ({:.1}s)", t1.elapsed().as_secs_f32()),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
