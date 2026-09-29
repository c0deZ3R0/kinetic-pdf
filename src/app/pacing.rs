//! Keeping frames in step with the display without OpenGL's vsync.
//!
//! On a laptop whose screen hangs off the integrated GPU while the discrete
//! one draws, each frame is copied from one to the other before the desktop
//! shows it, and with OpenGL's vsync on, NVIDIA's driver hands some of them
//! over out of order: scrolling, the page jumped backwards now and then --
//! 65 times in 999 captures of a scroll through a drawing set, and none with
//! vsync off. Drawing on the integrated GPU instead cured it but made heavy
//! sheets several times slower to zoom and pan, and speed is what this app is
//! for. So vsync is left off, and frames are kept in step with the display
//! here instead, from the desktop compositor's own timing
//! (`DwmGetCompositionTimingInfo`).
//!
//! A frame waits only when one has already been drawn since the display last
//! refreshed, and then only until the next refresh. One that comes late never
//! waits: waiting on the compositor outright (`DwmFlush`) cured the jumps too,
//! but a frame that ran late waited a whole refresh more, and one frame in
//! twenty took twice as long as it should have.
//!
//! Two more things keep it smooth without vsync's queue of frames, which used
//! to hide a frame that ran long. A moving view's drawing stops in time for
//! the frame to make the next refresh (`drawing_deadline`), so a frame that
//! runs long is rare rather than one in six. And a still view whose squares
//! are being drawn isn't paced at all (`App::ui`): nothing on screen moves,
//! each frame is full of drawing already, and pacing those frames made a zoom
//! half as long again to come sharp.

use std::time::Duration;

use windows_sys::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// How frames keep step with the display (`frame_sync`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameSync {
    /// Paced here, from the compositor's timing: the default.
    Paced,
    /// OpenGL's own vsync, with `KINETIC_PDF_VSYNC=1`.
    Vsync,
    /// Neither, with `KINETIC_PDF_VSYNC=0`: the benchmarks turn it off to see
    /// what the frame rate was hiding.
    Free,
}

impl FrameSync {
    pub fn label(self) -> &'static str {
        match self {
            FrameSync::Paced => "paced",
            FrameSync::Vsync => "on",
            FrameSync::Free => "off",
        }
    }
}

/// How frames keep step with the display in this run.
pub fn frame_sync() -> FrameSync {
    static SYNC: std::sync::OnceLock<FrameSync> = std::sync::OnceLock::new();
    *SYNC.get_or_init(|| match std::env::var("KINETIC_PDF_VSYNC").as_deref() {
        Ok("0") => FrameSync::Free,
        Ok("1") => FrameSync::Vsync,
        _ => FrameSync::Paced,
    })
}

/// The longest a frame is held back, whatever the timing says: a refresh at
/// 20 Hz, so a nonsense answer from the compositor can't stall the window.
const LONGEST_WAIT: Duration = Duration::from_millis(50);

/// Which of the display's refreshes the last frame was drawn in.
#[derive(Default)]
pub(super) struct Pacer {
    last: Option<i64>,
    /// When the last frame began, after any wait, and when the app had built
    /// it: what's left is painting it and handing it over.
    began: Option<std::time::Instant>,
    built: Option<std::time::Instant>,
    /// Steps of building a frame, for the trace of one that takes too long.
    marks: Vec<(&'static str, std::time::Instant)>,
    /// How far apart the display's refreshes are, as last seen.
    period: Option<Duration>,
}

/// What a frame leaves itself after its drawing budget, for painting what it
/// has built and handing it over: 1 to 3 ms of it went that way while
/// scrolling a drawing set.
const HAND_OVER: Duration = Duration::from_micros(2500);

/// The least drawing a moving frame gets, however late it is, so a view kept
/// moving still comes sharp in the end.
const LEAST_DRAWING: Duration = Duration::from_micros(500);

impl Pacer {
    /// When this frame's drawing has to stop to be shown at the next refresh,
    /// with room left to paint and hand it over -- never less than
    /// `LEAST_DRAWING` from now. `None` when frames aren't paced here.
    pub(super) fn drawing_deadline(&self) -> Option<std::time::Instant> {
        let (began, period) = (self.began?, self.period?);
        let now = std::time::Instant::now();
        let due = (began + period).checked_sub(HAND_OVER).unwrap_or(began);
        Some(due.max(now + LEAST_DRAWING))
    }

    /// A frame that doesn't wait, noted as `wait` notes one that does, so the
    /// frames after it are paced from where it was.
    pub(super) fn unpaced(&mut self) {
        let Some(timing) = Timing::now() else { return };
        self.period = Some(timing.period());
        self.last = Some(timing.refresh_at(timing.now));
        self.began = Some(std::time::Instant::now());
        self.marks.clear();
    }

    /// Notes that a step of building this frame is done.
    pub(super) fn mark(&mut self, step: &'static str) {
        self.marks.push((step, std::time::Instant::now()));
    }

    /// The app has built this frame; it is painted and handed over next.
    pub(super) fn ui_done(&mut self) {
        self.built = Some(std::time::Instant::now());
    }

    /// Waits, if a frame has already been drawn since the display last
    /// refreshed, until it next does. Doesn't wait if the compositor can't say.
    pub(super) fn wait(&mut self) {
        let arrived = std::time::Instant::now();
        let Some(timing) = Timing::now() else { return };
        let period = timing.period();
        self.period = Some(period);
        self.trace_if_slow(arrived, period);
        let mut refresh = timing.refresh_at(timing.now);
        if self.last.is_some_and(|last| last >= refresh) {
            refresh = self.last.unwrap_or(refresh) + 1;
            let wait = timing.until(refresh);
            if !wait.is_zero() {
                std::thread::sleep(wait.min(LONGEST_WAIT));
            }
        }
        self.last = Some(refresh);
        self.began = Some(std::time::Instant::now());
    }

    /// For the trace: a frame that took more than a refresh and a half, split
    /// into the steps of building it (`mark`), and painting and handing it
    /// over. How the stutters left after pacing were found and taken out.
    fn trace_if_slow(&mut self, arrived: std::time::Instant, period: Duration) {
        let marks = std::mem::take(&mut self.marks);
        let (Some(began), Some(built)) = (self.began, self.built) else { return };
        let took = arrived - began;
        if took <= period * 3 / 2 || built < began {
            return;
        }
        let mut at = began;
        let steps: Vec<String> = marks
            .iter()
            .map(|&(step, done)| {
                let spent = done.saturating_duration_since(at);
                at = done;
                format!("{step} {:.1}", spent.as_secs_f64() * 1000.0)
            })
            .collect();
        crate::worker::trace(format_args!(
            "pacing: a frame took {:.1} ms: {:.1} building it ({}), {:.1} painting and handing it over",
            took.as_secs_f64() * 1000.0,
            (built - began).as_secs_f64() * 1000.0,
            steps.join(", "),
            (arrived - built).as_secs_f64() * 1000.0
        ));
    }
}

/// The compositor's timing, in performance-counter ticks.
struct Timing {
    now: i64,
    /// The display's last refresh, and its number: refreshes are counted from
    /// the one number rather than from the last, which moves on each time.
    vblank: i64,
    count: i64,
    /// How far apart refreshes are.
    period: i64,
    frequency: i64,
}

impl Timing {
    fn now() -> Option<Timing> {
        let mut info: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<DWM_TIMING_INFO>() as u32;
        // For the whole desktop: a window's own timing isn't offered since
        // Windows 8.1.
        if unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut info) } < 0 {
            return None;
        }
        let (mut now, mut frequency) = (0i64, 0i64);
        unsafe {
            QueryPerformanceCounter(&mut now);
            QueryPerformanceFrequency(&mut frequency);
        }
        let period = i64::try_from(info.qpcRefreshPeriod).ok().filter(|&p| p > 0)?;
        let vblank = i64::try_from(info.qpcVBlank).ok()?;
        let count = i64::try_from(info.cRefresh).ok()?;
        (frequency > 0).then_some(Timing { now, vblank, count, period, frequency })
    }

    /// How far apart the display's refreshes are.
    fn period(&self) -> Duration {
        Duration::from_secs_f64(self.period as f64 / self.frequency as f64)
    }

    /// The number of the refresh `at` falls in.
    fn refresh_at(&self, at: i64) -> i64 {
        self.count + (at - self.vblank).div_euclid(self.period)
    }

    /// How long until refresh number `refresh` begins.
    fn until(&self, refresh: i64) -> Duration {
        let ticks = (self.vblank + (refresh - self.count) * self.period - self.now).max(0);
        Duration::from_secs_f64(ticks as f64 / self.frequency as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_waits_only_for_a_refresh_it_has_already_drawn_in() {
        // 100 ticks a refresh, 1,000 a second: 0.1 s a refresh. Refresh 40 was
        // at tick 1,000.
        let at = |now: i64| Timing { now, vblank: 1000, count: 40, period: 100, frequency: 1000 };
        assert_eq!(at(1050).refresh_at(1050), 40);
        assert_eq!(at(1150).refresh_at(1150), 41);
        assert_eq!(at(950).refresh_at(950), 39, "before the refresh the compositor gave");
        // Half way through refresh 40, the next begins 50 ticks on.
        assert_eq!(at(1050).until(41), Duration::from_millis(50));
        assert_eq!(at(1250).until(41), Duration::ZERO, "late: no waiting");
    }

    #[test]
    fn a_moving_frame_stops_drawing_in_time_for_the_next_refresh_but_always_draws_a_little() {
        let now = std::time::Instant::now();
        let period = Duration::from_micros(6944);
        let pacer = Pacer { began: Some(now), period: Some(period), ..Default::default() };
        let deadline = pacer.drawing_deadline().unwrap();
        assert!(deadline <= now + period - HAND_OVER + Duration::from_micros(100) && deadline > now, "room left to paint and hand over");
        // A frame already past its refresh still gets a sliver of drawing.
        let late = Pacer { began: Some(now - period * 2), period: Some(period), ..Default::default() };
        assert!(late.drawing_deadline().unwrap() >= now + LEAST_DRAWING);
        assert!(Pacer::default().drawing_deadline().is_none(), "not paced: no deadline");
    }

    #[test]
    fn frames_are_numbered_the_same_whichever_refresh_the_compositor_last_saw() {
        // The same moment, seen from refresh 40 and from refresh 42 two
        // periods later, is the same refresh: the count doesn't drift.
        let early = Timing { now: 1250, vblank: 1000, count: 40, period: 100, frequency: 1000 };
        let later = Timing { now: 1250, vblank: 1200, count: 42, period: 100, frequency: 1000 };
        assert_eq!(early.refresh_at(1250), later.refresh_at(1250));
        assert_eq!(early.until(43), later.until(43));
    }
}
