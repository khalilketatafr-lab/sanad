//! Sentinel — the anti-automation velocity model and response ladder
//! (blueprint 05 §5).
//!
//! Sentinel never sees content. It watches *behaviour*: how fast pages are
//! turned, how variable the dwell is, whether the page is visible, the input
//! modality, and a few hard automation tells (a WebDriver flag, a software
//! rasterizer on a datacenter IP). From a rolling [`SessionDigest`] it keeps a
//! risk level 0–4 and returns the [`Response`] the Kernel applies at the next
//! lease renewal.
//!
//! Escalation is gradual and mostly invisible (05 §5.2): level 1 only shrinks
//! the prefetch window, level 2 asks for an invisible attestation, level 3
//! paces renewals to a human ceiling, and only level 4 pauses the session for
//! human review. The calibration rule is the point of the whole design: the
//! thresholds come from real reading distributions so that **fewer than 0.1 %
//! of human sessions ever reach level 2**. Skimming and flipping to find a
//! figure are short, bursty and high-variance, and are explicitly allowed —
//! the robotic tell is *low* dwell variance at a sustained fast cadence, which
//! humans do not produce.
//!
//! The model is pure: `observe` is a deterministic function of the digest, the
//! attestation outcome and the prior state, so the ladder is unit-testable
//! without a Kernel, a database or a clock. Wiring the [`Response`] into the
//! lease window (the `ahead` prefetch) and the renewal pacing is done where the
//! session store lives.

/// Prefetch window at level 0, in chunks (matches `routes::editions::WINDOW`).
pub const NORMAL_AHEAD: u32 = 3;
/// A velocity judgement needs at least this many pages of evidence.
pub const MIN_PAGES_FOR_VELOCITY: u32 = 50;
/// Sustained cadence above this is "fast": 1 page / 2 s (05 §5.2, level 1).
pub const FAST_PAGES_PER_S: f64 = 0.5;
/// Dwell coefficient of variation below this is robotic; humans sit well above.
pub const ROBOTIC_DWELL_CV: f64 = 0.35;
/// Fast-and-robotic windows in a row before level 1 persists into level 2.
pub const L1_PERSIST_WINDOWS: u32 = 2;
/// Independent automation tells that together mean "clear automation" (level 4).
pub const CLEAR_AUTOMATION_TELLS: u32 = 3;
/// Concurrent sessions per account above this is itself one automation tell.
pub const MAX_HUMAN_CONCURRENCY: u32 = 3;

/// A behavioural digest for one evaluation window. No content, ever (05 §8):
/// every field is an aggregate or a boolean.
// The booleans are independent automation signals, not a state to model as an
// enum; they are deliberately flat so `automation_tells` can count them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct SessionDigest {
    /// Pages advanced in the window.
    pub pages: u32,
    /// Wall-clock span of the window, milliseconds.
    pub span_ms: u64,
    /// Coefficient of variation of per-page dwell (std/mean). Low = robotic.
    pub dwell_cv: f64,
    /// Fraction of the window the reader was visible (0..=1).
    pub visibility_ratio: f64,
    /// Frame-timing regularity, 0 = human jitter, 1 = metronomic.
    pub frame_regularity: f64,
    /// Only keyboard input was seen (no pointer or touch at all).
    pub keyboard_only: bool,
    /// `navigator.webdriver` was set.
    pub webdriver: bool,
    /// The GPU renderer class looks like a software rasterizer (headless).
    pub software_renderer: bool,
    /// The connection is from a known datacenter range.
    pub datacenter_ip: bool,
    /// Concurrent sessions on the account.
    pub concurrent_sessions: u32,
}

impl SessionDigest {
    /// A neutral, human-looking digest to build test or default cases from.
    #[must_use]
    pub fn human(pages: u32, span_ms: u64, dwell_cv: f64) -> Self {
        Self {
            pages,
            span_ms,
            dwell_cv,
            visibility_ratio: 1.0,
            frame_regularity: 0.2,
            keyboard_only: false,
            webdriver: false,
            software_renderer: false,
            datacenter_ip: false,
            concurrent_sessions: 1,
        }
    }

    /// Pages per second over the window (0 when the span is empty).
    #[must_use]
    pub fn pages_per_second(&self) -> f64 {
        if self.span_ms == 0 {
            return 0.0;
        }
        f64::from(self.pages) / (self.span_ms as f64 / 1000.0)
    }

    /// Sustained fast *and* low-variance over enough pages: the robotic tell.
    #[must_use]
    fn velocity_suspicious(&self) -> bool {
        self.pages >= MIN_PAGES_FOR_VELOCITY
            && self.pages_per_second() > FAST_PAGES_PER_S
            && self.dwell_cv < ROBOTIC_DWELL_CV
    }

    /// Count of independent automation tells present in the window.
    #[must_use]
    fn automation_tells(&self) -> u32 {
        u32::from(self.webdriver)
            + u32::from(self.software_renderer)
            + u32::from(self.datacenter_ip)
            + u32::from(self.frame_regularity > 0.9)
            + u32::from(self.keyboard_only)
            + u32::from(self.concurrent_sessions > MAX_HUMAN_CONCURRENCY)
            + u32::from(self.visibility_ratio < 0.5)
    }

    /// At least one strong tell that, with velocity, means automation (level 2).
    #[must_use]
    fn hard_automation(&self) -> bool {
        self.webdriver
            || (self.software_renderer && self.datacenter_ip)
            || self.frame_regularity > 0.9
    }

    /// A window that looks human: not fast-robotic, no hard tell, and visible.
    #[must_use]
    fn clean(&self) -> bool {
        !self.velocity_suspicious() && !self.hard_automation() && self.visibility_ratio >= 0.5
    }
}

/// Whether an invisible attestation was requested and how it resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attestation {
    NotRequested,
    Passed,
    Failed,
}

/// The risk level, 0 (normal) to 4 (clear automation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Normal,
    Watch,
    Challenge,
    Pace,
    Paused,
}

impl Level {
    #[must_use]
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Normal,
            1 => Self::Watch,
            2 => Self::Challenge,
            3 => Self::Pace,
            _ => Self::Paused,
        }
    }

    /// The numeric level, 0–4.
    #[must_use]
    pub fn rank(self) -> u8 {
        self as u8
    }
}

/// What the Kernel applies for a session at a given level (05 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    pub level: Level,
    /// Prefetch window in chunks: full at level 0, clamped to 1 once watching.
    pub ahead_chunks: u32,
    /// Require an invisible attestation on the next renewal.
    pub require_attestation: bool,
    /// Pace renewals to a human reading ceiling.
    pub pace_to_human: bool,
    /// Pause the session for human review; the account is notified.
    pub paused: bool,
}

impl Response {
    #[must_use]
    fn for_level(level: Level) -> Self {
        Self {
            level,
            ahead_chunks: if level == Level::Normal {
                NORMAL_AHEAD
            } else {
                1
            },
            require_attestation: matches!(level, Level::Challenge | Level::Pace),
            pace_to_human: matches!(level, Level::Pace | Level::Paused),
            paused: level == Level::Paused,
        }
    }
}

/// The per-session risk state. One [`Sentinel`] per active reading session.
#[derive(Debug, Clone)]
pub struct Sentinel {
    level: u8,
    l1_streak: u32,
}

impl Default for Sentinel {
    fn default() -> Self {
        Self::new()
    }
}

impl Sentinel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            level: 0,
            l1_streak: 0,
        }
    }

    #[must_use]
    pub fn level(&self) -> Level {
        Level::from_u8(self.level)
    }

    /// Reconstructs persisted state (for the session store). `rank` is 0..=4.
    #[must_use]
    pub fn restore(rank: u8, l1_streak: u32) -> Self {
        Self {
            level: rank.min(4),
            l1_streak,
        }
    }

    /// The persistable state: `(level rank 0..=4, level-1 streak)`.
    #[must_use]
    pub fn snapshot(&self) -> (u8, u32) {
        (self.level, self.l1_streak)
    }

    /// Folds one window into the risk state and returns the response to apply.
    ///
    /// Escalation is immediate to the level the evidence supports; de-escalation
    /// is gradual, one level per clean window, so a human who briefly skims and
    /// slows down returns to normal without ceremony, while a bot that pauses
    /// does not instantly shed its history.
    pub fn observe(&mut self, digest: &SessionDigest, attestation: Attestation) -> Response {
        if digest.velocity_suspicious() {
            self.l1_streak = self.l1_streak.saturating_add(1);
        } else {
            self.l1_streak = 0;
        }

        // The level the current evidence alone supports.
        let mut target: u8 = 0;
        if digest.velocity_suspicious() {
            target = 1;
        }
        if self.l1_streak >= L1_PERSIST_WINDOWS || digest.hard_automation() {
            target = target.max(2);
        }
        if digest.automation_tells() >= CLEAR_AUTOMATION_TELLS {
            target = 4;
        }
        if attestation == Attestation::Failed {
            target = target.max(3);
        }
        // A passed challenge means a real human is behind a fast tool: it drops
        // the session out of the challenge tier at once, unless the window is
        // clear automation (level 4), which a mere challenge cannot absolve.
        let proven_human = attestation == Attestation::Passed && target < 4;

        if proven_human {
            self.level = self.level.min(1);
            self.l1_streak = 0;
        } else if target >= self.level {
            self.level = target;
        } else if digest.clean() {
            // De-escalate gradually, one level per clean window.
            self.level = self.level.saturating_sub(1);
        }
        // Otherwise hold: ambiguous windows neither escalate nor forgive.

        Response::for_level(self.level())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A tiny deterministic PRNG so the calibration test is reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }
        /// Uniform f64 in [0, 1).
        fn unit(&mut self) -> f64 {
            f64::from(self.next_u32()) / f64::from(u32::MAX)
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.unit()
        }
    }

    /// A robotic session defined purely by cadence (fast, low-variance): no
    /// hard automation tell, so it must climb the ladder by persistence.
    fn bot(pages: u32, span_ms: u64, cv: f64) -> SessionDigest {
        SessionDigest::human(pages, span_ms, cv)
    }

    #[test]
    fn a_normal_reader_stays_at_level_zero() {
        let mut s = Sentinel::new();
        // 60 pages over 20 minutes, bursty dwell.
        let r = s.observe(
            &SessionDigest::human(60, 20 * 60 * 1000, 0.8),
            Attestation::NotRequested,
        );
        assert_eq!(r.level, Level::Normal);
        assert_eq!(r.ahead_chunks, NORMAL_AHEAD);
        assert!(!r.require_attestation && !r.paused);
    }

    #[test]
    fn a_fast_but_bursty_skimmer_is_allowed() {
        let mut s = Sentinel::new();
        // Faster than 1 page/2s, but high dwell variance (human skimming).
        let d = SessionDigest::human(80, 120 * 1000, 0.7); // ~0.67 pages/s, cv 0.7
        assert!(d.pages_per_second() > FAST_PAGES_PER_S);
        for _ in 0..5 {
            assert_eq!(
                s.observe(&d, Attestation::NotRequested).level,
                Level::Normal
            );
        }
    }

    #[test]
    fn sustained_robotic_cadence_climbs_the_ladder() {
        let mut s = Sentinel::new();
        let d = bot(80, 80 * 1000, 0.05); // 1 page/s, metronomic
        // First window: watch (level 1).
        assert_eq!(s.observe(&d, Attestation::NotRequested).level, Level::Watch);
        // Persists → challenge (level 2), which asks for attestation.
        let r = s.observe(&d, Attestation::NotRequested);
        assert_eq!(r.level, Level::Challenge);
        assert!(r.require_attestation);
        assert_eq!(r.ahead_chunks, 1);
        // Attestation fails → pace to human ceiling (level 3).
        let r = s.observe(&d, Attestation::Failed);
        assert_eq!(r.level, Level::Pace);
        assert!(r.pace_to_human);
    }

    #[test]
    fn passing_the_attestation_clears_the_challenge() {
        let mut s = Sentinel::new();
        let d = bot(80, 80 * 1000, 0.05);
        s.observe(&d, Attestation::NotRequested);
        assert_eq!(
            s.observe(&d, Attestation::NotRequested).level,
            Level::Challenge
        );
        // A genuine human behind a fast tool proves themselves.
        let r = s.observe(&d, Attestation::Passed);
        assert_eq!(r.level, Level::Watch);
    }

    #[test]
    fn clear_automation_jumps_to_paused() {
        let mut s = Sentinel::new();
        let d = SessionDigest {
            webdriver: true,
            software_renderer: true,
            datacenter_ip: true,
            visibility_ratio: 0.1,
            ..SessionDigest::human(200, 60 * 1000, 0.02)
        };
        let r = s.observe(&d, Attestation::NotRequested);
        assert_eq!(r.level, Level::Paused);
        assert!(r.paused);
    }

    #[test]
    fn a_stopped_bot_de_escalates_one_level_per_clean_window() {
        let mut s = Sentinel::new();
        let d = bot(80, 80 * 1000, 0.05);
        s.observe(&d, Attestation::Failed);
        s.observe(&d, Attestation::Failed);
        let paced = s.observe(&d, Attestation::Failed);
        assert_eq!(paced.level, Level::Pace);
        // Clean windows now: level steps down one at a time.
        let calm = SessionDigest::human(30, 15 * 60 * 1000, 0.9);
        assert_eq!(
            s.observe(&calm, Attestation::NotRequested).level,
            Level::Challenge
        );
        assert_eq!(
            s.observe(&calm, Attestation::NotRequested).level,
            Level::Watch
        );
        assert_eq!(
            s.observe(&calm, Attestation::NotRequested).level,
            Level::Normal
        );
    }

    #[test]
    fn calibration_fewer_than_one_in_a_thousand_humans_reach_challenge() {
        // Draw a realistic spread of human sessions — including fast skimmers —
        // and confirm the blueprint's rule: < 0.1% ever reach level 2.
        let mut rng = Lcg(0x5EED_1234_ABCD_0001);
        let n = 5000;
        let mut reached_challenge = 0;
        for _ in 0..n {
            let mut s = Sentinel::new();
            // A session is several windows of reading.
            let fast_skimmer = rng.unit() < 0.2;
            for _ in 0..6 {
                let pages = 40 + (rng.next_u32() % 90);
                // Readers: 4–40 s/page; skimmers: ~1.2–2.5 s/page but bursty.
                let per_page_ms = if fast_skimmer {
                    rng.range(1200.0, 2500.0)
                } else {
                    rng.range(4000.0, 40000.0)
                };
                let span_ms = (f64::from(pages) * per_page_ms) as u64;
                // Human dwell variance is high; skimming is even burstier.
                let cv = if fast_skimmer {
                    rng.range(0.6, 1.3)
                } else {
                    rng.range(0.45, 1.4)
                };
                let visibility = rng.range(0.7, 1.0);
                let d = SessionDigest {
                    visibility_ratio: visibility,
                    frame_regularity: rng.range(0.05, 0.5),
                    ..SessionDigest::human(pages, span_ms, cv)
                };
                if s.observe(&d, Attestation::NotRequested).level >= Level::Challenge {
                    reached_challenge += 1;
                    break;
                }
            }
        }
        let rate = f64::from(reached_challenge) / f64::from(n);
        assert!(
            rate < 0.001,
            "{reached_challenge}/{n} humans reached challenge ({rate})"
        );
    }

    #[test]
    fn a_patient_scraper_at_human_cadence_is_caught_by_its_tells_not_its_speed() {
        // Slow enough to beat the velocity model, but headless on a datacenter IP.
        let mut s = Sentinel::new();
        let d = SessionDigest {
            webdriver: true,
            ..SessionDigest::human(40, 30 * 60 * 1000, 0.9)
        };
        // webdriver is a hard tell → challenge, regardless of the calm cadence.
        assert_eq!(
            s.observe(&d, Attestation::NotRequested).level,
            Level::Challenge
        );
    }
}
