//! Reading JUnit XML test reports: one parser for every runner that writes them (node:test's
//! `--test-reporter=junit`, vitest's `--reporter=junit`, jest-junit, pytest's `--junitxml`).
//!
//! The test command prints its reports after the tests have run, between [`JUNIT_BEGIN`] and
//! [`JUNIT_END`], each under a [`JUNIT_FILE`] line naming the test file it came from (node:test's
//! report names no file). Only the last such block is read: whatever the code under test
//! prints comes before it, so a report a test prints itself never stands in for the real one.

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::testlog::{TestResults, TestStatus};

/// The lines around the reports in a test run's output.
pub const JUNIT_BEGIN: &str = "<<<NASIKO-JUNIT-XML";
pub const JUNIT_END: &str = "NASIKO-JUNIT-XML>>>";
/// Starts one test file's report; the rest of the line is the file.
pub const JUNIT_FILE: &str = "@@NASIKO-JUNIT-FILE ";

/// node:test's `classname` on every test case: it says nothing about the test.
const NODE_DEFAULT_CLASSNAME: &str = "test";

/// Reads the last block of reports. A test is named by its file, the suites around it and its
/// own name (`test/a.test.ts::outer::inner::works`); a `classname` that adds something (pytest's
/// `tests.test_a.TestA`) is kept too. A test reported more than once (one that passes and then
/// fails in teardown) keeps its worst status. A report cut short keeps the tests it finished.
pub fn parse(output: &str) -> TestResults {
    let mut results = TestResults::new();
    let Some(block) = last_block(output) else {
        return results;
    };
    for (file, xml) in sections(block) {
        read_report(file, xml, &mut results);
    }
    results
}

fn last_block(output: &str) -> Option<&str> {
    let start = output.rfind(JUNIT_BEGIN)? + JUNIT_BEGIN.len();
    let rest = &output[start..];
    Some(rest.find(JUNIT_END).map_or(rest, |end| &rest[..end]))
}

/// The block's reports, each with the file its [`JUNIT_FILE`] line names (`None` for a report
/// before any such line: a runner that wrote one report for every file).
fn sections(block: &str) -> Vec<(Option<&str>, &str)> {
    let mut out = Vec::new();
    let mut file = None;
    let mut start = 0;
    let mut at = 0;
    for line in block.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix(JUNIT_FILE) {
            out.push((file, &block[start..at]));
            file = Some(name.trim());
            start = at + line.len();
        }
        at += line.len();
    }
    out.push((file, &block[start..]));
    out.retain(|(_, xml)| !xml.trim().is_empty());
    out
}

/// The test case being read: its name and status so far.
struct OpenCase {
    name: String,
    status: TestStatus,
}

fn read_report(file: Option<&str>, xml: &str, results: &mut TestResults) {
    let mut reader = Reader::from_str(xml.trim());
    let mut suites: Vec<String> = Vec::new();
    let mut open: Option<OpenCase> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if is(&e, "testsuite") => {
                suites.push(attribute(&e, "name").unwrap_or_default());
            }
            Ok(Event::End(e)) if e.name().as_ref() == "testsuite" => {
                suites.pop();
            }
            Ok(Event::Start(e)) if is(&e, "testcase") => {
                open = Some(OpenCase {
                    name: case_name(file, &suites, &e),
                    status: TestStatus::Passed,
                });
            }
            Ok(Event::Empty(e)) if is(&e, "testcase") => {
                record(results, case_name(file, &suites, &e), TestStatus::Passed);
            }
            Ok(Event::End(e)) if e.name().as_ref() == "testcase" => {
                if let Some(case) = open.take() {
                    record(results, case.name, case.status);
                }
            }
            Ok(Event::Start(e) | Event::Empty(e)) => {
                if let (Some(case), Some(child)) = (open.as_mut(), child_status(&e)) {
                    case.status = worst(case.status, child);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fn is(element: &BytesStart<'_>, name: &str) -> bool {
    element.name().as_ref() == name
}

/// `file::suite::…::classname::name`, leaving out what repeats or says nothing: a suite named
/// after the file (vitest), and a `classname` that is node:test's default, the file, or the
/// innermost suite.
fn case_name(file: Option<&str>, suites: &[String], case: &BytesStart<'_>) -> String {
    let classname = attribute(case, "classname").unwrap_or_default();
    let name = attribute(case, "name").unwrap_or_default();
    let mut parts: Vec<&str> = file.into_iter().collect();
    parts.extend(
        suites
            .iter()
            .map(String::as_str)
            .filter(|s| !s.is_empty() && Some(*s) != file),
    );
    let adds_something = !classname.is_empty()
        && classname != NODE_DEFAULT_CLASSNAME
        && Some(classname.as_str()) != file
        && suites.last() != Some(&classname);
    if adds_something {
        parts.push(&classname);
    }
    parts.push(&name);
    parts.join("::")
}

/// The status a `testcase` child stands for: `failure` and `error` fail it, `skipped` (also a
/// todo or an expected failure) ignores it.
fn child_status(element: &BytesStart<'_>) -> Option<TestStatus> {
    match element.name().as_ref() {
        "failure" | "error" => Some(TestStatus::Failed),
        "skipped" => Some(TestStatus::Ignored),
        _ => None,
    }
}

fn worst(a: TestStatus, b: TestStatus) -> TestStatus {
    let rank = |s| match s {
        TestStatus::Passed => 0,
        TestStatus::Ignored => 1,
        TestStatus::Failed => 2,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn record(results: &mut TestResults, name: String, status: TestStatus) {
    let entry = results.entry(name).or_insert(status);
    *entry = worst(*entry, status);
}

fn attribute(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
        .map(|v| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use TestStatus::{Failed, Ignored, Passed};

    /// Real reports, written by node 24's test runner and vitest 3 for the same test file.
    const NODE_MIXED: &str = include_str!("testdata/junit/node-mixed.xml");
    const NODE_BROKEN: &str = include_str!("testdata/junit/node-broken.xml");
    const VITEST_MIXED: &str = include_str!("testdata/junit/vitest-mixed.xml");

    fn block(reports: &[(&str, &str)]) -> String {
        let mut out = format!("test output\n{JUNIT_BEGIN}\n");
        for (file, xml) in reports {
            out.push_str(&format!("{JUNIT_FILE}{file}\n{xml}\n"));
        }
        out.push_str(JUNIT_END);
        out.push('\n');
        out
    }

    #[test]
    fn node_test_reports_name_tests_by_file_and_nested_suites() {
        let r = parse(&block(&[("test/mixed.test.mjs", NODE_MIXED)]));
        let expected = [
            ("outer::inner::nested fails", Failed),
            ("outer::inner::nested passes", Passed),
            ("outer::same name", Passed),
            ("same name", Passed),
            ("skipped one", Ignored),
            ("todo one", Ignored),
            ("top level fails", Failed),
            ("top level passes", Passed),
        ]
        .map(|(n, s)| (format!("test/mixed.test.mjs::{n}"), s));
        assert_eq!(r.into_iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn vitest_reports_read_the_same_tests() {
        let r = parse(&block(&[("mixed.test.mjs", VITEST_MIXED)]));
        let get = |n: &str| r.get(&format!("mixed.test.mjs::{n}")).copied();
        assert_eq!(get("top level passes"), Some(Passed));
        assert_eq!(get("top level fails"), Some(Failed));
        assert_eq!(get("skipped one"), Some(Ignored));
        assert_eq!(get("todo one"), Some(Ignored));
        assert_eq!(get("outer > inner > nested fails"), Some(Failed));
        assert_eq!(get("outer > same name"), Some(Passed));
        assert_eq!(get("same name"), Some(Passed));
        assert_eq!(r.len(), 8);
    }

    #[test]
    fn a_file_that_cannot_load_fails_as_one_test() {
        let r = parse(&block(&[
            ("test/broken.test.mjs", NODE_BROKEN),
            ("test/mixed.test.mjs", NODE_MIXED),
        ]));
        assert_eq!(
            r.get("test/broken.test.mjs::broken.test.mjs"),
            Some(&Failed)
        );
        assert_eq!(
            r.get("test/mixed.test.mjs::top level passes"),
            Some(&Passed)
        );
    }

    #[test]
    fn same_named_tests_of_two_files_stay_apart() {
        let pass = r#"<testsuites><testcase name="works" classname="test"/></testsuites>"#;
        let fail = r#"<testsuites><testcase name="works" classname="test"><failure message="x"/></testcase></testsuites>"#;
        let r = parse(&block(&[
            ("test/a.test.ts", pass),
            ("test/b.test.ts", fail),
        ]));
        assert_eq!(r.get("test/a.test.ts::works"), Some(&Passed));
        assert_eq!(r.get("test/b.test.ts::works"), Some(&Failed));
    }

    #[test]
    fn a_report_printed_by_the_code_under_test_is_not_read() {
        let fake = r#"<testsuites><testcase name="works" classname="test"/></testsuites>"#;
        let real = r#"<testsuites><testcase name="works" classname="test"><failure message="no"/></testcase></testsuites>"#;
        let output = format!(
            "{}{}",
            block(&[("test/a.test.ts", fake)]),
            block(&[("test/a.test.ts", real)])
        );
        assert_eq!(parse(&output).get("test/a.test.ts::works"), Some(&Failed));
    }

    #[test]
    fn a_test_failing_in_teardown_after_passing_fails() {
        let xml = r#"<testsuite name="pytest"><testcase classname="tests.test_a" file="tests/test_a.py" name="test_x"/><testcase classname="tests.test_a" file="tests/test_a.py" name="test_x"><error message="error at teardown"/></testcase></testsuite>"#;
        let r = parse(&block(&[("tests/test_a.py", xml)]));
        assert_eq!(
            r.get("tests/test_a.py::pytest::tests.test_a::test_x"),
            Some(&Failed)
        );
    }

    #[test]
    fn no_report_or_a_broken_one_gives_only_the_finished_tests() {
        assert!(parse("ran nothing\n").is_empty());
        assert!(
            parse(&format!(
                "{JUNIT_BEGIN}\n{JUNIT_FILE}a.test.ts\n\n{JUNIT_END}"
            ))
            .is_empty()
        );
        let cut = format!(
            "{JUNIT_BEGIN}\n{JUNIT_FILE}a.test.ts\n<testsuites><testcase name=\"done\" classname=\"test\"/><testcase name=\"cut\" classname=\"test\"><fail"
        );
        let r = parse(&cut);
        assert_eq!(r.get("a.test.ts::done"), Some(&Passed));
        assert_eq!(r.get("a.test.ts::cut"), None);
    }

    #[test]
    fn one_report_for_every_file_needs_no_file_line() {
        let xml = r#"<testsuites name="vitest tests"><testsuite name="src/a.test.ts"><testcase classname="src/a.test.ts" name="works"/></testsuite></testsuites>"#;
        let output = format!("{JUNIT_BEGIN}\n{xml}\n{JUNIT_END}\n");
        assert_eq!(parse(&output).get("src/a.test.ts::works"), Some(&Passed));
    }
}
