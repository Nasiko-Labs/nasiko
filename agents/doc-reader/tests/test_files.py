import asyncio
import base64
import sys
from pathlib import Path
from types import SimpleNamespace

from a2a.types import Message, Part

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from main import DocReaderExecutor, extract_files_from_message, file_content_part

PNG = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
PDF = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"


def message(*parts: Part) -> Message:
    return Message(message_id="m1", parts=list(parts))


def test_a_raw_file_part_from_a2a_sdk_1_is_extracted():
    msg = message(
        Part(text="what is in the picture?"),
        Part(raw=PNG, filename="fact.png", media_type="image/png"),
        Part(raw=PDF, filename="q3.pdf", media_type="Application/PDF; name=q3.pdf"),
    )

    files = extract_files_from_message(msg)

    assert files == [
        {"filename": "fact.png", "media_type": "image/png", "data": PNG},
        {"filename": "q3.pdf", "media_type": "application/pdf", "data": PDF},
    ]


def test_parts_without_bytes_are_not_files():
    msg = message(Part(text="hi"), Part(url="https://example.com/a.png", media_type="image/png"))

    assert extract_files_from_message(msg) == []
    assert extract_files_from_message(None) == []


def test_a_nameless_untyped_file_still_gets_a_name_and_type():
    files = extract_files_from_message(message(Part(raw=b"abc")))

    assert files == [{"filename": "unnamed", "media_type": "application/octet-stream", "data": b"abc"}]


def test_an_image_reaches_the_model_as_an_image():
    part = file_content_part({"filename": "fact.png", "media_type": "image/png", "data": PNG})

    assert part == {
        "type": "image_url",
        "image_url": {"url": "data:image/png;base64," + base64.b64encode(PNG).decode()},
    }


def test_a_pdf_reaches_the_model_as_a_file():
    part = file_content_part({"filename": "q3.pdf", "media_type": "application/pdf", "data": PDF})

    assert part == {
        "type": "file",
        "file": {
            "filename": "q3.pdf",
            "file_data": "data:application/pdf;base64," + base64.b64encode(PDF).decode(),
        },
    }


def test_a_text_file_reaches_the_model_as_text():
    part = file_content_part({"filename": "rows.csv", "media_type": "text/csv", "data": b"a,b\n1,2\n"})

    assert part == {"type": "text", "text": "--- File: rows.csv (text/csv) ---\na,b\n1,2\n"}


def test_json_is_read_as_text():
    part = file_content_part({"filename": "x.json", "media_type": "application/json", "data": b'{"k": 1}'})

    assert part["type"] == "text"
    assert part["text"].endswith('{"k": 1}')


def test_a_long_text_file_is_cut_and_says_so():
    part = file_content_part({"filename": "big.txt", "media_type": "text/plain", "data": b"x" * 60_000})

    assert part["text"] == (
        "--- File: big.txt (text/plain) ---\n" + "x" * 50_000 + "\n\n... [truncated, 60000 total chars]"
    )


def test_text_that_is_not_utf8_is_named_instead_of_garbled():
    part = file_content_part({"filename": "bad.txt", "media_type": "text/plain", "data": b"\xff\xfe\x00bad"})

    assert part == {
        "type": "text",
        "text": "--- File: bad.txt (text/plain) was not read: it is not valid UTF-8 text ---",
    }


def test_an_unsupported_type_is_named_instead_of_garbled():
    for media_type in ("application/zip", "image/tiff", "application/octet-stream"):
        part = file_content_part({"filename": "f", "media_type": media_type, "data": b"\x00\x01"})

        assert part == {
            "type": "text",
            "text": f"--- File: f ({media_type}) was not read: this agent reads images "
            "(PNG, JPEG, GIF, WebP), PDFs and text files ---",
        }


class FakeLlm:
    def __init__(self):
        self.calls = []
        self.chat = SimpleNamespace(completions=SimpleNamespace(create=self.create))

    async def create(self, **kwargs):
        self.calls.append(kwargs)
        return SimpleNamespace(choices=[SimpleNamespace(message=SimpleNamespace(content="ok"))])


def test_files_go_in_the_user_turn_after_the_question():
    llm = FakeLlm()
    files = [
        {"filename": "fact.png", "media_type": "image/png", "data": PNG},
        {"filename": "rows.csv", "media_type": "text/csv", "data": b"a\n"},
    ]

    answer = asyncio.run(DocReaderExecutor(llm, "gpt-4o")._process("what colours?", files))

    assert answer == "ok"
    system, user = llm.calls[0]["messages"]
    assert system["role"] == "system"
    assert "a\n" not in system["content"]
    assert user["role"] == "user"
    assert user["content"][0] == {"type": "text", "text": "what colours?"}
    assert user["content"][1]["type"] == "image_url"
    assert user["content"][2]["text"].startswith("--- File: rows.csv")


def test_files_without_a_question_are_summarised():
    llm = FakeLlm()

    asyncio.run(
        DocReaderExecutor(llm, "gpt-4o")._process(
            "", [{"filename": "q3.pdf", "media_type": "application/pdf", "data": PDF}]
        )
    )

    user = llm.calls[0]["messages"][1]
    assert user["content"][0] == {"type": "text", "text": "Summarize the uploaded document(s)."}
    assert user["content"][1]["type"] == "file"


def test_no_files_is_a_plain_chat():
    llm = FakeLlm()

    asyncio.run(DocReaderExecutor(llm, "gpt-4o")._process("hello", []))

    assert llm.calls[0]["messages"][1] == {"role": "user", "content": "hello"}
