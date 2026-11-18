//! Seven separate browsers move their mouse at the same time, each from its
//! own task and each in its own way (its own path, speed, number of moves
//! and pauses, like different people), and none of them may stall while
//! another one is moving: the mouse is driven through
//! `Input.dispatchMouseEvent`, per page, so there is no shared OS cursor and
//! nothing to switch between browsers.
//!
//! Each page records the time (epoch ms, comparable across processes) at
//! which it handled every `mousemove`. Each browser knows the pause it asked
//! for before every move; an interruption is a gap between two moves that is
//! longer than that pause by more than `MAX_EXTRA_MS`. The test also checks
//! that every browser got every move, that the movements overlapped in time,
//! and that all together took about as long as the slowest one alone.
//!
//! Not part of the normal run: `cargo test -p oxidewright --test
//! mouse_parallel_browsers -- --ignored --nocapture`. `OXIDEWRIGHT_HEADED=1`
//! runs it with visible (overlapping) windows, like the workers; `OXIDEWRIGHT_BROWSERS` changes the number of browsers.

use std::time::{Duration, Instant};

use oxidewright::{BrowserContext, Page, chromium};

/// A gap this much longer than the planned pause is an interruption.
const MAX_EXTRA_MS: f64 = 250.0;

const PAGE: &str = r#"<body style="margin:0;width:100vw;height:100vh">
<script>
  window.moves = [];
  document.addEventListener('mousemove', e => {
    window.moves.push([performance.timeOrigin + performance.now(), e.clientX, e.clientY, e.isTrusted]);
  });
</script></body>"#;

fn profile(index: usize) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("oxw-mouse-{}-{index}", std::process::id()))
}

fn browser_count() -> usize {
    std::env::var("OXIDEWRIGHT_BROWSERS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(7)
}

/// One browser with its own profile; its startup window is the only one.
async fn launch(index: usize) -> BrowserContext {
    let mut launch = chromium().launch_persistent_context(profile(index));
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    if std::env::var("OXIDEWRIGHT_HEADED").is_ok_and(|v| v == "1") {
        launch = launch.headless(false);
    }
    launch.await.expect("failed to launch the browser")
}

/// One move of a plan: where to go and how long to wait before going.
#[derive(Clone, Copy)]
struct Step {
    pause: Duration,
    x: f64,
    y: f64,
}

/// How one browser moves: its own seed gives it its own strokes (curves
/// between random points), its own speed, and its own hesitations.
fn plan(seed: u64) -> Vec<Step> {
    let mut rng = fastrand::Rng::with_seed(seed);
    // Per browser: how fast this "hand" is (ms between moves) and how often it hesitates.
    let base_pause = rng.u64(6..=30);
    let jitter = rng.u64(0..=base_pause / 2);
    let hesitate_every = rng.usize(25..=90);
    let strokes = rng.usize(6..=14);

    let mut steps = Vec::new();
    let (mut x, mut y) = (rng.f64() * 700.0 + 50.0, rng.f64() * 450.0 + 50.0);
    for _ in 0..strokes {
        let (to_x, to_y) = (rng.f64() * 700.0 + 50.0, rng.f64() * 450.0 + 50.0);
        // A quadratic curve with a random control point; points are denser
        // at both ends (ease in and out), like a real hand.
        let (cx, cy) = (rng.f64() * 800.0, rng.f64() * 550.0);
        let points = rng.usize(15..=60);
        for i in 1..=points {
            let t = i as f64 / points as f64;
            let t = t * t * (3.0 - 2.0 * t);
            let u = 1.0 - t;
            let px = u * u * x + 2.0 * u * t * cx + t * t * to_x;
            let py = u * u * y + 2.0 * u * t * cy + t * t * to_y;
            let mut pause = base_pause + rng.u64(0..=jitter);
            if steps.len() % hesitate_every == hesitate_every - 1 {
                pause += rng.u64(150..=700);
            }
            let (px, py) = (px.round(), py.round());
            // Chrome sends no `mousemove` for a move to the same pixel.
            if steps.last().is_some_and(|s: &Step| s.x == px && s.y == py) {
                continue;
            }
            steps.push(Step {
                pause: Duration::from_millis(pause),
                x: px,
                y: py,
            });
        }
        // Small pause between strokes.
        if let Some(last) = steps.last_mut() {
            last.pause += Duration::from_millis(rng.u64(40..=250));
        }
        (x, y) = (to_x, to_y);
    }
    steps
}

struct Run {
    plan: Vec<Step>,
    /// (time, x, y, trusted) of every `mousemove` of the plan, as the page
    /// handled it.
    moves: Vec<(f64, f64, f64, bool)>,
    /// `mousemove`s the page got that the test did not send (the real mouse
    /// passing over a visible window).
    outside: usize,
    /// Longest time one `mouse.move(...).await` took on the Rust side.
    slowest_call: Duration,
    took: Duration,
}

async fn move_in(seed: u64, page: Page, mut start: tokio::sync::watch::Receiver<bool>) -> Run {
    page.set_content(PAGE).await.unwrap();
    let mouse = page.mouse();
    let plan = plan(seed);
    start.wait_for(|go| *go).await.unwrap();

    let began = Instant::now();
    let mut slowest_call = Duration::ZERO;
    for step in &plan {
        tokio::time::sleep(step.pause).await;
        let call = Instant::now();
        mouse.move_to(step.x, step.y).await.unwrap();
        slowest_call = slowest_call.max(call.elapsed());
    }
    let took = began.elapsed();
    let all: Vec<(f64, f64, f64, bool)> = page
        .evaluate("() => window.moves")
        .isolated_context(false)
        .await
        .unwrap();
    // Keep the events of the plan, in order; anything else came from outside.
    let mut moves = Vec::new();
    let mut next = plan.iter().peekable();
    for event in &all {
        if next
            .peek()
            .is_some_and(|s| s.x == event.1 && s.y == event.2)
        {
            next.next();
            moves.push(*event);
        }
    }
    Run {
        outside: all.len() - moves.len(),
        plan,
        moves,
        slowest_call,
        took,
    }
}

/// Move the mouse in all `browsers` at once (browser `i` with seed `seeds[i]`);
/// returns each run and the time from the common start until the last one finished.
async fn move_together(pages: &[Page], seeds: &[u64]) -> (Vec<Run>, Duration) {
    let (go, start) = tokio::sync::watch::channel(false);
    let tasks: Vec<_> = pages
        .iter()
        .zip(seeds)
        .map(|(p, &seed)| tokio::spawn(move_in(seed, p.clone(), start.clone())))
        .collect();
    // Let every page load before the movement starts together.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let began = Instant::now();
    go.send(true).unwrap();
    let mut runs = Vec::new();
    for task in tasks {
        runs.push(task.await.unwrap());
    }
    (runs, began.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opens seven browsers for ~3 minutes: run it with `-- --ignored`"]
async fn seven_browsers_move_the_mouse_at_the_same_time_without_stalls() {
    let count = browser_count();
    let seeds: Vec<u64> = (1..=count as u64).collect();
    let browsers: Vec<BrowserContext> = futures::future::join_all((0..count).map(launch)).await;
    let pages: Vec<Page> = browsers.iter().map(|b| b.pages()[0].clone()).collect();

    // First each browser's movement alone, one after the other, to know how
    // long each takes on this machine; then all of them together.
    let mut alone = Vec::new();
    for (page, &seed) in pages.iter().zip(&seeds) {
        let (runs, _) = move_together(std::slice::from_ref(page), &[seed]).await;
        alone.push(runs[0].took);
    }
    let (runs, wall) = move_together(&pages, &seeds).await;
    for (i, browser) in browsers.iter().enumerate() {
        let _ = browser.close().await;
        let _ = std::fs::remove_dir_all(profile(i));
    }

    let slowest_alone = alone.iter().max().copied().unwrap_or_default();
    let sum_alone: Duration = alone.iter().sum();
    println!(
        "{count} browsers together: {:.2} s; the slowest one alone: {:.2} s; one after the other: {:.2} s",
        wall.as_secs_f64(),
        slowest_alone.as_secs_f64(),
        sum_alone.as_secs_f64()
    );
    println!(
        "browser  moves  outside  trusted  alone(s)  together(s)  median gap  longest pause  max extra  slowest call"
    );
    let mut failures = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        let n = i + 1;
        // Gap between moves k-1 and k, minus the pause planned before move k.
        let mut extras: Vec<f64> = run
            .moves
            .windows(2)
            .zip(run.plan.iter().skip(1))
            .map(|(w, step)| w[1].0 - w[0].0 - step.pause.as_secs_f64() * 1000.0)
            .collect();
        let mut gaps: Vec<f64> = run.moves.windows(2).map(|w| w[1].0 - w[0].0).collect();
        gaps.sort_by(f64::total_cmp);
        extras.sort_by(f64::total_cmp);
        let median_gap = gaps.get(gaps.len() / 2).copied().unwrap_or(0.0);
        let max_extra = extras.last().copied().unwrap_or(0.0);
        let longest_pause = run.plan.iter().map(|s| s.pause).max().unwrap_or_default();
        let trusted = run.moves.iter().filter(|m| m.3).count();
        println!(
            "{n:>7}  {:>5}  {:>7}  {:>7}  {:>8.2}  {:>11.2}  {:>8.1}ms  {:>11}ms  {:>7.1}ms  {:>10.1}ms",
            run.moves.len(),
            run.outside,
            trusted,
            alone[i].as_secs_f64(),
            run.took.as_secs_f64(),
            median_gap,
            longest_pause.as_millis(),
            max_extra,
            run.slowest_call.as_secs_f64() * 1000.0
        );
        if run.moves.len() != run.plan.len() {
            failures.push(format!(
                "browser {n} got {} of {} moves",
                run.moves.len(),
                run.plan.len()
            ));
        }
        if trusted != run.moves.len() {
            failures.push(format!(
                "browser {n}: {} moves were not trusted",
                run.moves.len() - trusted
            ));
        }
        if max_extra > MAX_EXTRA_MS {
            failures.push(format!(
                "browser {n} stopped {max_extra:.0} ms longer than planned between two moves"
            ));
        }
    }

    // Overlap: the last browser to start started before the first one to
    // finish finished, i.e. at some point all of them were moving.
    let latest_first = runs
        .iter()
        .filter_map(|r| r.moves.first().map(|m| m.0))
        .fold(f64::MIN, f64::max);
    let earliest_last = runs
        .iter()
        .filter_map(|r| r.moves.last().map(|m| m.0))
        .fold(f64::MAX, f64::min);
    if latest_first >= earliest_last {
        failures.push("the browsers did not all move at the same time".into());
    }
    // Not taking turns: all together take about as long as the slowest alone.
    if wall.as_secs_f64() > slowest_alone.as_secs_f64() * 1.25 + 1.0 {
        failures.push(format!(
            "all together took {:.2} s, the slowest alone {:.2} s",
            wall.as_secs_f64(),
            slowest_alone.as_secs_f64()
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
