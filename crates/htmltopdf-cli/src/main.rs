use std::{
    env, fs,
    path::PathBuf,
    process,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use htmltopdf::{Engine, FontSource, RenderOptions};

fn main() {
    if let Err(error) = run() {
        eprintln!("htmltopdf: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    if args.first().is_some_and(|arg| arg == "bench") {
        return run_sequential_benchmark(&args[1..]);
    }

    if args.first().is_some_and(|arg| arg == "bench-concurrent") {
        return run_concurrent_benchmark(&args[1..]);
    }

    // Optional `--font <path-or-family>` and `--js` may appear before/among the
    // positionals.
    let mut positionals: Vec<std::ffi::OsString> = Vec::new();
    let mut font: Option<String> = None;
    let mut scripting = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--font" {
            let value = iter
                .next()
                .ok_or_else(|| "--font requires a font file path or family name".to_string())?;
            font = Some(value.to_string_lossy().into_owned());
        } else if arg == "--js" {
            scripting = true;
        } else {
            positionals.push(arg);
        }
    }

    if positionals.len() != 2 {
        return Err(
            concat!(
                "usage: htmltopdf [--font <path|family>] [--js] <input.html> <output.pdf>\n",
                "       htmltopdf bench <input.html> <output-dir> [runs]\n",
                "       htmltopdf bench-concurrent <input.html> <output-dir> <workers> <runs-per-worker>"
            )
                .to_string(),
        );
    }

    let input_path = PathBuf::from(&positionals[0]);
    let output_path = PathBuf::from(&positionals[1]);
    let html = fs::read_to_string(&input_path)
        .map_err(|error| format!("failed to read {}: {error}", input_path.display()))?;

    let mut options = build_options(font.as_deref())?;
    // Resolve relative <img src> paths against the input file's directory.
    if let Some(parent) = input_path.parent() {
        if !parent.as_os_str().is_empty() {
            options = options.with_base_dir(parent);
        }
    }
    let pdf = render(&html, options, scripting)
        .map_err(|error| format!("failed to render {}: {error}", input_path.display()))?;

    fs::write(&output_path, pdf)
        .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;

    Ok(())
}

/// Render, optionally running the bounded pre-layout JavaScript stage when `--js`
/// is given (requires the `js` build feature).
#[cfg(feature = "js")]
fn render(html: &str, options: RenderOptions, scripting: bool) -> Result<Vec<u8>, String> {
    let engine = Engine::new();
    if scripting {
        engine
            .render_html_with_scripts(
                html,
                options,
                &htmltopdf::BoaScriptEngine,
                &htmltopdf::ScriptLimits::default(),
            )
            .map_err(|error| error.to_string())
    } else {
        engine.render_html(html, options).map_err(|e| e.to_string())
    }
}

#[cfg(not(feature = "js"))]
fn render(html: &str, options: RenderOptions, scripting: bool) -> Result<Vec<u8>, String> {
    if scripting {
        return Err(
            "this build has no JavaScript support; rebuild with `--features js` to use --js"
                .to_string(),
        );
    }
    Engine::new()
        .render_html(html, options)
        .map_err(|error| error.to_string())
}

/// Build render options, loading an embedded font when `--font` was given. A
/// value that names an existing file is treated as a font path; otherwise it is
/// resolved as a system font family name.
fn build_options(font: Option<&str>) -> Result<RenderOptions, String> {
    let Some(font) = font else {
        return Ok(RenderOptions::default());
    };

    let source = if std::path::Path::new(font).is_file() {
        FontSource::Path(PathBuf::from(font))
    } else {
        FontSource::Family(font.to_string())
    };

    RenderOptions::default().with_font(&source)
}

fn run_sequential_benchmark(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("usage: htmltopdf bench <input.html> <output-dir> [runs]".to_string());
    }

    let input_path = PathBuf::from(&args[0]);
    let output_dir = PathBuf::from(&args[1]);
    let runs = args
        .get(2)
        .map(|value| {
            value
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|error| format!("invalid run count: {error}"))
        })
        .transpose()?
        .unwrap_or(10);

    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("failed to create {}: {error}", output_dir.display()))?;

    let html = fs::read_to_string(&input_path)
        .map_err(|error| format!("failed to read {}: {error}", input_path.display()))?;
    let engine = Engine::new();
    let mut total = Duration::ZERO;
    let mut output_bytes = 0;

    for index in 0..runs {
        let started = Instant::now();
        let pdf = engine
            .render_html(&html, RenderOptions::default())
            .map_err(|error| format!("failed to render {}: {error}", input_path.display()))?;
        let elapsed = started.elapsed();
        total += elapsed;
        output_bytes = pdf.len();

        let output_path = output_dir.join(format!("bench-{index}.pdf"));
        fs::write(&output_path, pdf)
            .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    }

    let average = total / runs as u32;
    println!("input: {}", input_path.display());
    println!("runs: {runs}");
    println!("total_ms: {:.3}", total.as_secs_f64() * 1000.0);
    println!("avg_ms: {:.3}", average.as_secs_f64() * 1000.0);
    println!("last_output_bytes: {output_bytes}");

    Ok(())
}

fn run_concurrent_benchmark(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() != 4 {
        return Err(
            "usage: htmltopdf bench-concurrent <input.html> <output-dir> <workers> <runs-per-worker>"
                .to_string(),
        );
    }

    let input_path = PathBuf::from(&args[0]);
    let output_dir = PathBuf::from(&args[1]);
    let workers = parse_positive_usize(&args[2], "workers")?;
    let runs_per_worker = parse_positive_usize(&args[3], "runs-per-worker")?;

    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("failed to create {}: {error}", output_dir.display()))?;

    let html = Arc::new(
        fs::read_to_string(&input_path)
            .map_err(|error| format!("failed to read {}: {error}", input_path.display()))?,
    );
    let started = Instant::now();
    let mut handles = Vec::with_capacity(workers);

    for worker in 0..workers {
        let html = Arc::clone(&html);
        let output_dir = output_dir.clone();

        handles.push(thread::spawn(move || -> Result<WorkerResult, String> {
            let engine = Engine::new();
            let mut output_bytes = 0;

            for run in 0..runs_per_worker {
                let pdf = engine
                    .render_html(&html, RenderOptions::default())
                    .map_err(|error| format!("worker {worker} render failed: {error}"))?;
                output_bytes = pdf.len();

                let output_path = output_dir.join(format!("worker-{worker}-run-{run}.pdf"));
                fs::write(&output_path, pdf).map_err(|error| {
                    format!(
                        "worker {worker} failed to write {}: {error}",
                        output_path.display()
                    )
                })?;
            }

            Ok(WorkerResult { output_bytes })
        }));
    }

    let mut last_output_bytes = 0;
    for handle in handles {
        let result = handle
            .join()
            .map_err(|_| "benchmark worker panicked".to_string())??;
        last_output_bytes = result.output_bytes;
    }

    let elapsed = started.elapsed();
    let total_runs = workers * runs_per_worker;
    let average = elapsed / total_runs as u32;

    println!("input: {}", input_path.display());
    println!("workers: {workers}");
    println!("runs_per_worker: {runs_per_worker}");
    println!("total_runs: {total_runs}");
    println!("wall_ms: {:.3}", elapsed.as_secs_f64() * 1000.0);
    println!("avg_wall_ms_per_pdf: {:.3}", average.as_secs_f64() * 1000.0);
    println!("last_output_bytes: {last_output_bytes}");

    Ok(())
}

fn parse_positive_usize(value: &std::ffi::OsString, label: &str) -> Result<usize, String> {
    let parsed = value
        .to_string_lossy()
        .parse::<usize>()
        .map_err(|error| format!("invalid {label}: {error}"))?;

    if parsed == 0 {
        return Err(format!("{label} must be greater than zero"));
    }

    Ok(parsed)
}

struct WorkerResult {
    output_bytes: usize,
}
