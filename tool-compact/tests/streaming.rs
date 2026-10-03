//! Chunk boundaries must not change decoding: every split point, and one character at a time,
//! over a corpus of replies that includes failures.

mod common;

use common::tools;
use nasiko_tool_compact::{CompactError, Decoded, StreamDecoder, decode_reply};

fn corpus() -> Vec<&'static str> {
    vec![
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
        "I'll do both.\n<<call ping>>\n<<call ping>>\nDone.",
        r#"<<call send_email {"to":["sam@x.io"],"subject":"a >> b","body":"x"}>>"#,
        r#"<<call send_email {"to":["s@x.io"],"subject":"}>> \" \\ <<call","body":"😀 会议"}>>"#,
        "no calls here, just < and << and <<ca and <<callback",
        "<<<call ping>>",
        "trailing marker prefix <<cal",
        "<<call\tping\n>>",
        r#"<<call tracker.create_ticket {"title":"t","priority":"high","assignee":{"name":"D"}}>>"#,
        // failures
        r#"<<call delete_everything {}>>"#,
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30"}"#,
        r#"<<call ping {"a":1,}>>"#,
        "<<call>>",
        "<<call ping",
        r#"ok <<call ping>> then <<call nope>>"#,
    ]
}

/// Feed `parts` in order and gather text and calls (or the error).
fn run(parts: &[&str]) -> Result<Decoded, CompactError> {
    let mut decoder = StreamDecoder::new(&tools())?;
    let mut text = String::new();
    for part in parts {
        text.push_str(&decoder.push(part)?);
    }
    let rest = decoder.finish()?;
    text.push_str(&rest.text);
    Ok(Decoded {
        text,
        calls: rest.calls,
    })
}

#[test]
fn every_split_point_gives_the_same_result_as_one_chunk() {
    for reply in corpus() {
        let whole = decode_reply(reply, &tools());
        for (i, _) in reply.char_indices().skip(1) {
            let (a, b) = reply.split_at(i);
            assert_eq!(run(&[a, b]), whole, "reply {reply:?} split at {i}");
        }
    }
}

#[test]
fn one_character_chunks_give_the_same_result_as_one_chunk() {
    for reply in corpus() {
        let whole = decode_reply(reply, &tools());
        let chars: Vec<String> = reply.chars().map(String::from).collect();
        let parts: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(run(&parts), whole, "reply {reply:?}");
    }
}

#[test]
fn every_pair_of_split_points_gives_the_same_result() {
    // Three-way splits catch state that only goes wrong when a boundary lands inside a marker
    // *and* inside the JSON of the same call.
    for reply in corpus().into_iter().take(4) {
        let whole = decode_reply(reply, &tools());
        let cuts: Vec<usize> = reply.char_indices().map(|(i, _)| i).skip(1).collect();
        for (n, &i) in cuts.iter().enumerate() {
            for &j in &cuts[n + 1..] {
                let parts = [&reply[..i], &reply[i..j], &reply[j..]];
                assert_eq!(run(&parts), whole, "reply {reply:?} split at {i},{j}");
            }
        }
    }
}

#[test]
fn the_public_sample_split_decodes() {
    let parts = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let decoded = run(&parts).unwrap();
    assert_eq!(decoded.calls.len(), 1);
    assert_eq!(decoded.calls[0].arguments["title"], "Retro");
    assert_eq!(decoded.text, "");
}

#[test]
fn text_is_released_before_the_reply_ends() {
    let mut decoder = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(decoder.push("Hello <").unwrap(), "Hello ");
    assert_eq!(decoder.push("<x").unwrap(), "<<x");
    assert_eq!(decoder.push(" <<call ping>> bye").unwrap(), "  bye");
    assert_eq!(decoder.finish().unwrap().calls.len(), 1);
}
