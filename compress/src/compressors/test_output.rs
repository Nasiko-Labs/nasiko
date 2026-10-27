//! Keeps everything a failing test run says about the failure; drops what says nothing.
//!
//! A test report is mostly noise around a small, critical core. The noise is the per-test
//! "passed" lines of a verbose run, progress dots with no failure in them, and setup chatter a
//! runner repeats once per worker (Django's `Cloning test database for alias 'default'...`).
//! The core is every failure block, traceback, assertion message and the summary line — and
//! those are exactly what an agent re-runs the suite to read, so none of it is ever cut.
//!
//! The rules are deliberately narrow: a line goes only if it matches one of three shapes. A line
//! this module does not recognise is kept, so an unfamiliar runner degrades to "unchanged",
//! never to "the error message is gone".

use super::log::render;
use crate::policy::Policy;

/// Fewer dropped lines than this and the marker costs about what it saves.
const MIN_DROPPED: usize = 3;

pub(super) fn compress(input: &str, policy: &Policy<'_>) -> Option<String> {
    let lines: Vec<&str> = input.lines().collect();
    let keep: Vec<bool> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let repeat = i > 0 && !line.trim().is_empty() && lines[i - 1] == *line;
            !(repeat || is_passing_line(line) || is_clean_progress(line))
        })
        .collect();

    let dropped = keep.iter().filter(|k| !**k).count();
    if dropped < MIN_DROPPED {
        return None;
    }
    Some(render(&lines, &keep, policy))
}

/// One passing test (unittest/Django verbose `test_x (mod.Case) ... ok`, Django migration
/// `Applying app.0001_initial... OK`, pytest verbose `path::test_x PASSED [ 10%]`).
fn is_passing_line(line: &str) -> bool {
    let t = line.trim_end();
    t.ends_with("... ok")
        || t.ends_with("... OK")
        || (t.contains("::") && t.split_whitespace().any(|w| w == "PASSED"))
}

/// Progress that records no failure: unittest's `....s..` or pytest's `tests/a.py ..s.  [ 40%]`.
/// A line containing `F` or `E` is a failure position and is kept.
fn is_clean_progress(line: &str) -> bool {
    let t = line.trim_end();
    let dots = match t.rsplit_once(" [") {
        // pytest: `<path> <dots>  [ NN%]`
        Some((head, pct)) if pct.ends_with("%]") => match head.trim_end().rsplit_once(' ') {
            Some((_, dots)) => dots,
            None => return false,
        },
        _ => t,
    };
    !dots.is_empty() && dots.contains('.') && dots.chars().all(|c| c == '.' || c == 's')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Level;

    fn policy() -> Policy<'static> {
        Policy {
            enabled: true,
            level: Level::Conservative,
            ..Default::default()
        }
    }

    /// A failing Django run, shaped like `runtests.py` output from the SWE-bench eval traces.
    fn django_run() -> String {
        let mut s = String::from(
            "Testing against Django installed in '/repo/django' with up to 8 processes\n\
             Found 120 test(s).\n\
             Creating test database for alias 'default'...\n",
        );
        for _ in 0..8 {
            s.push_str("Cloning test database for alias 'default'...\n");
        }
        for i in 0..40 {
            s.push_str(&format!(
                "test_case_{i} (queries.tests.QueryTests) ... ok\n"
            ));
        }
        s.push_str(
            "test_pickle (queries.tests.QueryTests) ... ERROR\n\
             \n\
             ======================================================================\n\
             ERROR: test_pickle (queries.tests.QueryTests)\n\
             ----------------------------------------------------------------------\n\
             Traceback (most recent call last):\n  \
             File \"/repo/tests/queries/tests.py\", line 88, in test_pickle\n    \
             qs2 = pickle.loads(pickle.dumps(qs))\n\
             AttributeError: 'NoneType' object has no attribute 'attname'\n\
             \n\
             ----------------------------------------------------------------------\n\
             Ran 120 tests in 2.311s\n\
             \n\
             FAILED (errors=1)\n",
        );
        for _ in 0..8 {
            s.push_str("Destroying test database for alias 'default'...\n");
        }
        s
    }

    #[test]
    fn keeps_every_line_of_the_failure_and_the_summary() {
        let input = django_run();
        let out = compress(&input, &policy()).unwrap();

        for needle in [
            "test_pickle (queries.tests.QueryTests) ... ERROR",
            "ERROR: test_pickle (queries.tests.QueryTests)",
            "Traceback (most recent call last):",
            "line 88, in test_pickle",
            "qs2 = pickle.loads(pickle.dumps(qs))",
            "AttributeError: 'NoneType' object has no attribute 'attname'",
            "Ran 120 tests in 2.311s",
            "FAILED (errors=1)",
            "Found 120 test(s).",
        ] {
            assert!(out.contains(needle), "lost {needle:?}:\n{out}");
        }
        assert!(out.len() < input.len());
    }

    #[test]
    fn drops_passing_tests_and_collapses_repeated_chatter() {
        let out = compress(&django_run(), &policy()).unwrap();

        assert!(
            !out.contains("test_case_7 (queries"),
            "passing line kept:\n{out}"
        );
        assert_eq!(
            out.matches("Cloning test database for alias 'default'...")
                .count(),
            1,
            "a repeated line must survive exactly once:\n{out}"
        );
        assert!(
            out.contains("lines elided"),
            "every drop leaves a counted marker"
        );
    }

    #[test]
    fn pytest_verbose_keeps_failures_section_and_drops_passed() {
        let mut input = String::from(
            "============================= test session starts ==============================\n\
             collected 30 items\n\n",
        );
        for i in 0..28 {
            input.push_str(&format!(
                "tests/test_api.py::test_ok_{i} PASSED [ {i:>2}%]\n"
            ));
        }
        input.push_str(
            "tests/test_api.py::test_broken FAILED [ 96%]\n\n\
             =================================== FAILURES ===================================\n\
             _________________________________ test_broken __________________________________\n\
             \n    def test_broken():\n>       assert add(2, 2) == 5\n\
             E       assert 4 == 5\n\n\
             tests/test_api.py:41: AssertionError\n\
             =========================== short test summary info ============================\n\
             FAILED tests/test_api.py::test_broken - assert 4 == 5\n\
             ========================= 1 failed, 28 passed in 0.42s =========================\n",
        );

        let out = compress(&input, &policy()).unwrap();

        assert!(!out.contains("test_ok_5 PASSED"));
        for needle in [
            "test_broken FAILED",
            ">       assert add(2, 2) == 5",
            "E       assert 4 == 5",
            "tests/test_api.py:41: AssertionError",
            "FAILED tests/test_api.py::test_broken - assert 4 == 5",
            "1 failed, 28 passed in 0.42s",
        ] {
            assert!(out.contains(needle), "lost {needle:?}:\n{out}");
        }
    }

    #[test]
    fn progress_dots_go_unless_they_mark_a_failure() {
        assert!(is_clean_progress("..........s....."));
        assert!(is_clean_progress(
            "tests/test_a.py ....s...                [ 40%]"
        ));
        assert!(!is_clean_progress("....F..E.."));
        assert!(!is_clean_progress(
            "tests/test_a.py ..F.                    [ 40%]"
        ));
        assert!(!is_clean_progress("Traceback (most recent call last):"));
    }

    #[test]
    fn a_short_report_with_nothing_to_drop_is_left_alone() {
        let input = "Ran 1 test in 0.001s\n\nOK\n";
        assert_eq!(compress(input, &policy()), None);
    }

    #[test]
    fn is_deterministic() {
        let input = django_run();
        assert_eq!(compress(&input, &policy()), compress(&input, &policy()));
    }
}
