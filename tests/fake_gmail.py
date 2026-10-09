#!/usr/bin/env python3
"""A pretend Gmail IMAP server for trying Unibox without real mail.

Speaks just enough IMAP (plus Gmail's X-GM-* extensions) for the client, over
plain TCP on 127.0.0.1:1143. Any address logs in with any password except "wrong".

    python3 tests/fake_gmail.py &
    UNIBOX_IMAP=plain:127.0.0.1:1143 target/debug/unibox --serve --data /tmp/unibox-test
"""
import re
import socketserver
import struct
import sys
import threading
import time
import zlib
from email.message import EmailMessage
from email.utils import format_datetime
from datetime import datetime, timezone

NOW = int(time.time())


def png(w, h, rgb):
    """A solid-colour PNG, built by hand so the test server needs nothing beyond the standard library."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = (b"\x00" + bytes(rgb) * w) * h
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


PNG = png(64, 32, (230, 120, 40))
LOCK = threading.Lock()


class Msg:
    def __init__(self, n, thrid, raw, when, inbox=True, seen=False, starred=False):
        self.msgid = str(1800000000000000000 + n)
        self.thrid = str(1800000000000000000 + thrid)
        self.raw = raw
        self.when = when
        self.inbox = inbox
        self.flags = set()
        if seen:
            self.flags.add("\\Seen")
        if starred:
            self.flags.add("\\Flagged")
        self.uid_all = n
        self.uid_inbox = n + 1000


def build(frm, to, subject, when, text=None, html=None, attach=None, inline=False):
    m = EmailMessage()
    m["From"], m["To"], m["Subject"] = frm, to, subject
    m["Date"] = format_datetime(datetime.fromtimestamp(when, timezone.utc))
    m.set_content(text or "See the HTML version.")
    if html:
        m.add_alternative(html, subtype="html")
        if inline:
            m.get_payload()[1].add_related(PNG, "image", "png", cid="<chart1@fake>")
    if attach:
        m.add_attachment(attach[1], maintype="application", subtype="pdf", filename=attach[0])
    return m.as_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")


def mailbox(owner):
    out, n = [], 0

    def add(thrid, hours_ago, **kw):
        nonlocal n
        n += 1
        flags = {k: kw.pop(k) for k in ("inbox", "seen", "starred") if k in kw}
        when = NOW - int(hours_ago * 3600)
        kw.setdefault("to", owner)
        out.append(Msg(n, thrid, build(when=when, **kw), when, **flags))

    if owner.startswith("work"):
        add(1, 0.3, frm="Priya Raman <priya@example.com>", subject="Q4 planning doc",
            html="<p>Hi,</p><p>Draft is <a href='https://example.com/doc'>here</a>. Comments by Friday please.</p>")
        add(2, 5, frm="Build Bot <ci@example.com>", subject="[ci] main is green", text="All 412 tests passed.", seen=True)
        add(3, 30, frm="HR <hr@example.com>", subject="Benefits enrolment closes soon", seen=True,
            html="<h2 style='color:#0a5'>Open enrolment</h2><p>Closes on the 20th.</p>",
            attach=("benefits-2027.pdf", b"%PDF-1.4 fake benefits"))
        for i in range(8):
            add(10 + i, 40 + i * 9, frm=f"Jira <jira{i}@example.com>", subject=f"[OPS-{410 + i}] Ticket updated",
                text=f"Ticket OPS-{410 + i} moved to In Review.", seen=i > 1)
        return out

    # A conversation: an old archived message, a reply the owner sent, and two in the inbox.
    add(1, 50, frm="Aoife Byrne <aoife@example.com>", subject="Dinner on Saturday?", inbox=False, seen=True,
        text="Are you two free on Saturday? Thinking of that place in Ginza.")
    add(1, 49, frm=f"Me <{owner}>", to="Aoife Byrne <aoife@example.com>", subject="Re: Dinner on Saturday?",
        inbox=False, seen=True, text="Yes! 7pm works. I'll book.")
    add(1, 3, frm="Aoife Byrne <aoife@example.com>", subject="Re: Dinner on Saturday?", seen=True,
        text="Perfect. Can you make it for five? Niamh is in town.")
    add(1, 0.2, frm="Niamh Byrne <niamh@example.com>", subject="Re: Dinner on Saturday?",
        html="<div>See you all there &#127843;</div><blockquote>Perfect. Can you make it for five?</blockquote>")
    add(2, 1, frm="Matt Levine <noreply@news.example.com>", subject="Money Stuff: Fake Newsletter Edition",
        html="""<html><head><style>.h{font:700 28px Georgia,serif}</style></head><body style="background:#ffffff">
        <table width="600" align="center" cellpadding="0" cellspacing="0"><tr><td>
        <div class="h">Money Stuff</div><p style="color:#333">Everything is <b>securities fraud</b>, again.</p>
        <p><img src="cid:chart1@fake" width="64" height="32" alt="chart"></p>
        <img src="https://tracker.invalid/open.gif" width="1" height="1">
        <script>document.title = "pwned"</script>
        <p><a href="https://example.com/article" onclick="steal()">Read on the web</a></p>
        </td></tr></table></body></html>""", inline=True)
    add(3, 6, frm="=?utf-8?b?44Os44K044K544Kv44O844Or?= <lego@example.jp>", subject="来週のレッスンについて",
        text="来週土曜日のレッスンは14:05からです。", starred=True)
    add(4, 26, frm="Billing <billing@example.com>", subject="Your invoice for October", seen=True,
        html="<p>Your invoice is attached.</p>", attach=("invoice-2026-10.pdf", b"%PDF-1.4 fake invoice"))
    for i in range(70):
        add(100 + i, 30 + i * 7, frm=f"Sender {i} <sender{i}@example.com>", subject=f"Routine message number {i}",
            text=f"This is the body of routine message {i}. " * 4, seen=i % 3 != 0)
    return out


BOXES = {}


def box(user):
    if user not in BOXES:
        BOXES[user] = mailbox(user)
    return BOXES[user]


def parse_set(s, uids):
    want = set()
    top = max(uids) if uids else 0
    for part in s.split(","):
        a, _, b = part.partition(":")
        lo = top if a == "*" else int(a)
        hi = lo if not b else (top if b == "*" else int(b))
        want.update(u for u in uids if min(lo, hi) <= u <= max(lo, hi))
    return want


class Handler(socketserver.StreamRequestHandler):
    def send(self, data):
        self.wfile.write(data if isinstance(data, bytes) else data.encode())

    def handle(self):
        self.send("* OK Gimap ready (fake)\r\n")
        user, selected = None, None
        while True:
            line = self.rfile.readline()
            if not line:
                return
            line = line.decode("utf-8", "replace").rstrip("\r\n")
            tag, _, rest = line.partition(" ")
            cmd, _, args = rest.partition(" ")
            cmd = cmd.upper()
            if cmd == "UID":
                sub, _, args = args.partition(" ")
                cmd = "UID " + sub.upper()
            with LOCK:
                if cmd == "LOGIN":
                    u, p = re.findall(r'"((?:[^"\\]|\\.)*)"', args)
                    if p == "wrong":
                        self.send(f"{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
                        continue
                    user = u
                    self.send(f"{tag} OK {u} authenticated (Success)\r\n")
                elif cmd == "LOGOUT":
                    self.send(f"* BYE\r\n{tag} OK done\r\n")
                    return
                elif cmd == "LIST":
                    self.send('* LIST (\\HasNoChildren) "/" "INBOX"\r\n'
                              '* LIST (\\HasChildren \\Noselect) "/" "[Gmail]"\r\n'
                              '* LIST (\\All \\HasNoChildren) "/" "[Gmail]/All Mail"\r\n'
                              f"{tag} OK Success\r\n")
                elif cmd in ("SELECT", "EXAMINE"):
                    selected = "inbox" if args.strip('"').upper() == "INBOX" else "all"
                    msgs = self.visible(user, selected)
                    self.send(f"* FLAGS (\\Seen \\Flagged \\Deleted)\r\n* {len(msgs)} EXISTS\r\n"
                              f"* OK [UIDVALIDITY 7] UIDs valid\r\n{tag} OK [READ-WRITE] {args} selected. (Success)\r\n")
                elif cmd == "UID FETCH":
                    setstr, _, items = args.partition(" ")
                    msgs = self.visible(user, selected)
                    want = parse_set(setstr, list(msgs))
                    for seq, uid in enumerate(sorted(msgs), 1):
                        if uid in want:
                            self.fetch(seq, uid, msgs[uid], items.upper())
                    self.send(f"{tag} OK Success\r\n")
                elif cmd == "UID SEARCH":
                    msgs = self.visible(user, selected)
                    thrids = set(re.findall(r"X-GM-THRID (\d+)", args))
                    if thrids:
                        hits = [u for u, x in msgs.items() if x.thrid in thrids]
                    elif "has:attachment" in args:
                        hits = [u for u, x in msgs.items() if b"filename=" in x.raw]
                    else:
                        hits = list(msgs)
                    self.send("* SEARCH " + " ".join(map(str, sorted(hits))) + f"\r\n{tag} OK SEARCH completed (Success)\r\n")
                elif cmd == "UID STORE":
                    setstr, op, flags = args.split(" ", 2)
                    msgs = self.visible(user, selected)
                    for uid in parse_set(setstr, list(msgs)):
                        for f in re.findall(r"\\\w+", flags):
                            (msgs[uid].flags.add if op.startswith("+") else msgs[uid].flags.discard)(f)
                    self.send(f"{tag} OK Success\r\n")
                elif cmd == "UID MOVE":
                    setstr, _, dest = args.partition(" ")
                    msgs = self.visible(user, selected)
                    if selected == "inbox" and "All Mail" in dest:
                        for uid in parse_set(setstr, list(msgs)):
                            msgs[uid].inbox = False
                        self.send(f"{tag} OK [COPYUID 7 {setstr} {setstr}] (Success)\r\n")
                    else:
                        self.send(f"{tag} NO [TRYCREATE] No folder {dest} (Failure)\r\n")
                else:
                    self.send(f"{tag} OK {cmd} completed\r\n")

    @staticmethod
    def visible(user, selected):
        msgs = box(user)
        if selected == "inbox":
            return {m.uid_inbox: m for m in msgs if m.inbox}
        return {m.uid_all: m for m in msgs}

    def fetch(self, seq, uid, m, items):
        date = datetime.fromtimestamp(m.when, timezone.utc).strftime("%d-%b-%Y %H:%M:%S +0000")
        head = (f'* {seq} FETCH (X-GM-THRID {m.thrid} X-GM-MSGID {m.msgid} '
                f'X-GM-LABELS ("\\\\Important" "Travel Plans") UID {uid} RFC822.SIZE {len(m.raw)} '
                f'INTERNALDATE "{date}" FLAGS ({" ".join(sorted(m.flags))})')
        if "BODY.PEEK[]" in items:
            self.send(head.encode() + b" BODY[] {%d}\r\n" % len(m.raw) + m.raw + b")\r\n")
        elif "HEADER.FIELDS" in items:
            hdr = m.raw.split(b"\r\n\r\n", 1)[0] + b"\r\n\r\n"
            self.send(head.encode() + b" BODY[HEADER.FIELDS (FROM TO CC SUBJECT)] {%d}\r\n" % len(hdr) + hdr + b")\r\n")
        else:
            self.send(head + ")\r\n")


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 1143
    print(f"fake Gmail on 127.0.0.1:{port}", flush=True)
    Server(("127.0.0.1", port), Handler).serve_forever()
