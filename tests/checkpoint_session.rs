//! A render session follows a chain of edits. After every edit, whatever range
//! is asked for must equal that range of a fresh render of the edited plan,
//! bit for bit, whatever the session kept, dropped or thinned.

use maac::dsp::{DspEngine, RenderSession, Reuse, SessionPolicy, ESTIMATED_CHECKPOINT_BYTES};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

const SPACING: u64 = 4_801;

fn limits() -> PlanLimits {
    PlanLimits::song()
}

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn render_full(plan: &PlanArtifact) -> Vec<f64> {
    let mut out = Vec::new();
    DspEngine::new_artifact_with_limits(plan, &limits())
        .unwrap()
        .render(|frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    out
}

fn rendered(
    session: &mut RenderSession,
    start: u64,
    end: u64,
) -> (Vec<f64>, maac::dsp::RenderReport) {
    let mut out = Vec::new();
    let report = session
        .render(start, end, |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    (out, report)
}

fn assert_range(session: &mut RenderSession, start: u64, end: u64, context: &str) {
    let plan = session.plan().clone();
    assert_range_of(session, &plan, start, end, context);
}

/// The session's render of `start..end` equals that range of a fresh render
/// of `plan`, which the test holds itself rather than asking the session.
fn assert_range_of(
    session: &mut RenderSession,
    plan: &PlanArtifact,
    start: u64,
    end: u64,
    context: &str,
) {
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(plan);
    let (got, report) = rendered(session, start, end);
    assert_eq!(
        bits(&fresh[start as usize * channels..end as usize * channels]),
        bits(&got),
        "{context}: range {start}..{end}, {report:?}"
    );
    assert_ascending(session, context);
}

fn assert_ascending(session: &RenderSession, context: &str) {
    let frames = session.checkpoint_frames();
    assert!(
        frames.windows(2).all(|pair| pair[0] < pair[1]),
        "{context}: checkpoints not strictly ascending: {frames:?}"
    );
}

#[derive(Default)]
struct Tally {
    renders: usize,
    resumed: usize,
    full: usize,
    unchanged: usize,
}

fn chain(seed: u64, edits: usize, policy: SessionPolicy, tally: &mut Tally) {
    let mut song = base_song(seed);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), policy).unwrap();
    let mut rng = Rng(seed ^ 0x5EED_5EED_0BAD_CAFE);
    // The first render takes every checkpoint.
    assert_range(&mut session, 0, total, &format!("seed {seed} first"));
    for index in 0..edits {
        let (next, kind) = edit(&song, &mut rng, seed + index as u64);
        let Ok(plan) = compile(&next) else { continue };
        song = next;
        session.update(&plan);
        let mut start = rng.below(total - 1);
        // A third of the time the range begins on a checkpoint the session
        // holds, which is where its comparisons are exact.
        let held = session.checkpoint_frames();
        if rng.below(3) == 0 && !held.is_empty() {
            start = held[rng.below(held.len() as u64) as usize];
        }
        // Half the time only a short stretch is rendered, which leaves the old
        // checkpoints after it in the session: the render that follows must
        // not trust them.
        let reach = if rng.below(2) == 0 { 20_000 } else { total };
        let mut end = (start + 1 + rng.below(reach)).min(total);
        if rng.below(3) == 0 {
            // End exactly on a checkpoint frame of the grid, when one follows.
            let on_grid =
                (start / policy.spacing_frames + 1 + rng.below(3)) * policy.spacing_frames;
            if on_grid > start && on_grid <= total {
                end = on_grid;
            }
        }
        let context = format!("seed {seed} edit {index} {kind}");
        let mut out = Vec::new();
        let report = session
            .render(start, end, |frame| {
                out.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        let channels = usize::from(plan.output().channels);
        let fresh = render_full(&plan);
        assert_eq!(
            bits(&fresh[start as usize * channels..end as usize * channels]),
            bits(&out),
            "{context}: range {start}..{end}, {report:?}"
        );
        assert!(
            report.started_at <= start,
            "{context}: started at {} after the range began at {start}",
            report.started_at
        );
        assert!(
            session.checkpoint_frames().len() <= policy_max(&policy),
            "{context}: over budget"
        );
        // Then a render with no edit in between, from anywhere.
        let again = rng.below(total - 1);
        let (again_end, context) = (total.min(again + 1 + rng.below(total)), context + " again");
        let (got, report_again) = rendered(&mut session, again, again_end);
        assert_eq!(
            bits(&fresh[again as usize * channels..again_end as usize * channels]),
            bits(&got),
            "{context}: range {again}..{again_end}, {report_again:?}"
        );
        tally.renders += 1;
        match report.reuse {
            Reuse::Unchanged => tally.unchanged += 1,
            Reuse::Full(_) => tally.full += 1,
            Reuse::From(_) => {}
        }
        tally.resumed += usize::from(report.started_at > 0);
        assert_ascending(&session, &context);
    }
}

fn policy_max(policy: &SessionPolicy) -> usize {
    (policy.budget_bytes / ESTIMATED_CHECKPOINT_BYTES).max(1) as usize
}

#[test]
fn a_session_follows_a_chain_of_edits_exactly() {
    let mut tally = Tally::default();
    for seed in 1..=4 {
        chain(seed, 6, SessionPolicy::new(SPACING), &mut tally);
    }
    // A spacing that divides the edit grid puts checkpoints exactly on the
    // bounds that edits report.
    for seed in 8..=10 {
        chain(seed, 6, SessionPolicy::new(6_000), &mut tally);
    }
    assert!(
        tally.renders >= 10,
        "too few edits compiled: {}",
        tally.renders
    );
    assert!(
        tally.resumed * 3 >= tally.renders,
        "sessions resumed in only {} of {} renders ({} full, {} unchanged)",
        tally.resumed,
        tally.renders,
        tally.full,
        tally.unchanged
    );
}

#[test]
fn a_session_thinned_to_a_few_checkpoints_is_still_exact() {
    let mut tally = Tally::default();
    let tight = SessionPolicy::new(SPACING).with_budget_bytes(3 * ESTIMATED_CHECKPOINT_BYTES);
    for seed in 5..=7 {
        chain(seed, 5, tight, &mut tally);
    }
    assert!(
        tally.renders >= 8,
        "too few edits compiled: {}",
        tally.renders
    );
}

#[test]
fn several_edits_before_one_render_cost_one_comparison() {
    let mut song = base_song(11);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(SPACING)).unwrap();
    assert_range(&mut session, 0, total, "first");
    let mut rng = Rng(99);
    let mut edits = 0;
    for index in 0..40 {
        let (next, _) = edit(&song, &mut rng, 11 + index);
        if let Ok(plan) = compile(&next) {
            song = next;
            session.update(&plan);
            edits += 1;
            if edits == 3 {
                break;
            }
        }
    }
    assert_eq!(edits, 3);
    // The reference is the last plan, held here; the session must have kept
    // the last update, not the first, and resumes late.
    let last = compile(&song).unwrap();
    assert_range_of(
        &mut session,
        &last,
        total * 3 / 4,
        total,
        "after three edits",
    );
    assert_range_of(&mut session, &last, 0, total, "again from the start");
}

#[test]
fn a_failed_render_leaves_the_session_as_it_was() {
    let song = base_song(21);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(SPACING)).unwrap();
    assert_range(&mut session, 0, total, "first");
    let frames = session.checkpoint_frames();
    assert!(frames.len() > 10);

    let mut rng = Rng(5);
    let edited = (0..40)
        .find_map(|index| compile(&edit(&song, &mut rng, 21 + index).0).ok())
        .expect("no edit compiled");
    session.update(&edited);
    let mut seen = 0;
    let failed = session.render(0, total, |_| {
        seen += 1;
        if seen > 20_000 {
            Err(maac::dsp::RenderError::RenderState("stop".into()))
        } else {
            Ok(())
        }
    });
    assert!(failed.is_err());
    assert_eq!(
        session.checkpoint_frames(),
        frames,
        "a failed render changed the checkpoints"
    );
    assert_range(&mut session, 0, total, "after the failure");
}

#[test]
fn a_late_edit_resumes_late() {
    let song = base_song(31);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(SPACING)).unwrap();
    assert_range(&mut session, 0, total, "first");
    // Move the last note's velocity: only the end of the song can change.
    let mut late = song.clone();
    let last = late
        .notes
        .iter_mut()
        .max_by_key(|note| note.at)
        .expect("a note");
    last.velocity = if last.velocity == 5 { 6 } else { 5 };
    let plan = compile(&late).expect("a velocity change compiles");
    session.update(&plan);
    // The listener is past the edit: the render may start from a checkpoint
    // near it, not from the beginning.
    let (_, report) = rendered(&mut session, total * 3 / 4, total);
    assert!(
        report.started_at >= total / 2,
        "a late edit started the render at {} of {total}: {report:?}",
        report.started_at
    );
    assert_range(&mut session, 0, total, "after the late edit");
}

#[test]
fn a_range_outside_the_plan_is_refused() {
    let plan = compile(&base_song(1)).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(SPACING)).unwrap();
    assert!(session.render(10, 10, |_| Ok(())).is_err());
    assert!(session.render(0, total + 1, |_| Ok(())).is_err());
    assert!(RenderSession::new(&plan, &limits(), SessionPolicy::new(8)).is_err());
}

/// A change a note list cannot make: another tempo. The session must start
/// again from frame 0 and say why, even when asked for a late range.
#[test]
fn a_change_to_the_tempo_renders_from_the_start() {
    let song = base_song(41);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(SPACING)).unwrap();
    assert_range(&mut session, 0, total, "first");

    let text = source(&song).replace("120bpm", "121bpm");
    let faster = maac::compiler::compile_bundle_artifact_with_limits(
        &maac::SourceBundle::new("main.maac", text),
        &limits(),
    )
    .unwrap();
    let total = faster.output().total_frames;
    session.update(&faster);
    let (_, report) = rendered(&mut session, total * 3 / 4, total);
    assert!(matches!(report.reuse, Reuse::Full(_)), "{report:?}");
    assert_eq!(report.started_at, 0);
    assert_range_of(
        &mut session,
        &faster,
        total * 3 / 4,
        total,
        "after the tempo change",
    );
}

/// Two channels, a range that begins between checkpoints, and a second range
/// on the unchanged plan.
#[test]
fn a_stereo_song_renders_ranges_exactly() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/basic/full_band.maac");
    let bundle =
        maac::bundle_fs::load_bundle(&path, std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
    let plan = maac::compiler::compile_bundle_artifact_with_limits(&bundle, &limits()).unwrap();
    assert_eq!(plan.output().channels, 2);
    let total = plan.output().total_frames.min(60_000 * 4);
    let mut session = RenderSession::new(&plan, &limits(), SessionPolicy::new(10_007)).unwrap();
    assert_range_of(&mut session, &plan, 0, total, "first");
    for (start, end) in [(30_001, 50_000), (10_007, 20_014), (59_999, total)] {
        assert_range_of(&mut session, &plan, start, end, "a later range");
    }
}

/// A tight spacing over a long range must not hold more checkpoints than the
/// budget while rendering.
#[test]
fn a_tight_spacing_is_widened_to_the_budget() {
    let plan = compile(&base_song(51)).unwrap();
    let total = plan.output().total_frames;
    let policy = SessionPolicy::new(32).with_budget_bytes(4 * ESTIMATED_CHECKPOINT_BYTES);
    let mut session = RenderSession::new(&plan, &limits(), policy).unwrap();
    let (_, report) = rendered(&mut session, 0, total);
    assert!(report.checkpoints <= 4, "{report:?}");
    assert_range(&mut session, 0, total, "after widening");
}
