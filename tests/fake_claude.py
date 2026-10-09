#!/usr/bin/env python3
"""A pretend Claude API for trying the assistant without a key or real mail leaving the PC.

It answers POST /v1/messages with scripted tool calls chosen from keywords in the person's
last message, so the whole loop can be exercised: query -> show -> act -> draft.

    python3 tests/fake_claude.py &          # listens on 127.0.0.1:1144
    then in the assistant's settings: endpoint http://127.0.0.1:1144, any key

Keywords: "jira" (find and show), "archive" (archive some of them), "draft" (reply to the
open conversation), "inject" (shows that mail content is passed back as data), anything
else gets a plain answer.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

JIRA = "SELECT message_id FROM mail WHERE from_addr LIKE '%jira%' AND in_inbox = 1"
CALLS = []  # every request body, for the test to inspect


def tool(name, **inp):
    return {"type": "tool_use", "id": f"toolu_{len(CALLS)}_{name}", "name": name, "input": inp}


def text(s):
    return {"type": "text", "text": s}


def last_person_message(messages):
    """(index, text, app_state) of the newest message the person typed."""
    for i in range(len(messages) - 1, -1, -1):
        m = messages[i]
        blocks = m["content"] if isinstance(m["content"], list) else [{"type": "text", "text": m["content"]}]
        if m["role"] == "user" and not any(b.get("type") == "tool_result" for b in blocks):
            texts = [b["text"] for b in blocks if b.get("type") == "text"]
            return i, texts[-1], texts[0] if len(texts) > 1 else ""
    return 0, "", ""


def results_since(messages, start):
    out = []
    for m in messages[start + 1:]:
        if m["role"] == "user" and isinstance(m["content"], list):
            out += [b for b in m["content"] if b.get("type") == "tool_result"]
    return out


def respond(body):
    messages = body["messages"]
    at, said, state = last_person_message(messages)
    results = results_since(messages, at)
    step = len(results)
    low = said.lower()

    if "jira" in low and "archive" not in low:
        if step == 0:
            return [text("Let me look."), tool("query_mail", sql=JIRA.replace("message_id", "message_id, subject"))], "tool_use"
        if step == 1:
            return [tool("show_conversations", title="Jira notifications", sql=JIRA)], "tool_use"
        n = len(json.loads(results[0]["content"])["rows"])
        return [text(f"I'm showing **{n}** Jira notifications from your inbox.\n\n- all from Jira\n- none older than a month")], "end_turn"

    if "archive" in low:
        if step == 0:
            return [tool("apply_action", action="archive", sql=JIRA + " AND subject LIKE '%OPS-41%'")], "tool_use"
        return [text("Done: " + str(results[0]["content"]))], "end_turn"

    if "draft" in low:
        open_conv = json.loads(state[state.index("{"):state.rindex("}") + 1]).get("open") if "{" in state else None
        if not open_conv:
            return [text("Open the conversation you want me to reply to first.")], "end_turn"
        if step == 0:
            return [tool("read_conversation", account_id=open_conv["account_id"], thread_id=open_conv["thread_id"])], "tool_use"
        if step == 1:
            return [tool("draft_reply", account_id=open_conv["account_id"], thread_id=open_conv["thread_id"],
                         body="お世話になっております。\n\nご連絡ありがとうございます。土曜日で問題ございません。\n\nよろしくお願いいたします。")], "tool_use"
        return [text(str(results[1]["content"]))], "end_turn"

    if "inject" in low:
        if step == 0:
            return [tool("query_mail", sql="SELECT message_id, body_text FROM mail WHERE subject LIKE 'Money Stuff%' LIMIT 1")], "tool_use"
        return [text("That email contains a script tag; I'm treating it as text.")], "end_turn"

    if "bad sql" in low:
        if step == 0:
            return [tool("query_mail", sql="DELETE FROM messages")], "tool_use"
        return [text("The app refused: " + str(results[0]["content"])[:80])], "end_turn"

    return [text(f"You said: {said}")], "end_turn"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        CALLS.append({"headers": dict(self.headers), "body": body})
        if self.headers.get("x-api-key") == "wrong":
            return self.reply(401, {"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}})
        content, stop = respond(body)
        self.reply(200, {"id": f"msg_{len(CALLS)}", "type": "message", "role": "assistant", "model": body["model"],
                         "content": content, "stop_reason": stop,
                         "usage": {"input_tokens": 100, "output_tokens": 20}})

    def do_GET(self):
        # the test reads back what the app sent
        self.reply(200, CALLS)

    def reply(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 1144
    print(f"fake Claude on 127.0.0.1:{port}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
