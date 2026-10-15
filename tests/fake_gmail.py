#!/usr/bin/env python3
"""A pretend Gmail IMAP server for trying Unibox without real mail.

Speaks just enough IMAP (plus Gmail's X-GM-* extensions) for the client, over
plain TCP on 127.0.0.1:1143. Any address logs in with any password except "wrong".

    python3 tests/fake_gmail.py &
    UNIBOX_IMAP=plain:127.0.0.1:1143 target/debug/unibox --serve --data /tmp/unibox-test
"""
import os
import re
import select
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
    def __init__(self, n, thrid, raw, when, inbox=True, seen=False, starred=False, draft=False):
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
        if draft:
            self.flags.add("\\Draft")
        self.uid_all = n
        self.uid_inbox = n + 1000


def auth_results(frm, to, auth):
    """The verdict header Gmail puts on top of incoming mail. Mail the owner sent has none."""
    domain = frm.rsplit("@", 1)[-1].strip(">")
    if auth is None:
        return b""
    lines = {
        "pass": [f"dkim=pass header.i=@{domain} header.s=s1 header.b=AbCd",
                 f"spf=pass (google.com: domain of bounce@{domain} designates 192.0.2.1 as permitted sender) smtp.mailfrom=bounce@{domain}",
                 f"dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from={domain}"],
        "fail": ["spf=softfail (google.com: domain of transitioning x@mailer.invalid does not designate 192.0.2.9 as permitted sender) smtp.mailfrom=x@mailer.invalid",
                 f"dmarc=fail (p=QUARANTINE sp=QUARANTINE dis=NONE) header.from={domain}"],
        "via": ["dkim=pass header.i=@bulkmailer.example header.s=s1 header.b=AbCd",
                "spf=pass (google.com: domain of b@bulkmailer.example designates 192.0.2.5 as permitted sender) smtp.mailfrom=b@bulkmailer.example"],
    }[auth]
    return ("Authentication-Results: mx.google.com;\r\n       " + ";\r\n       ".join(lines) + "\r\n").encode()


def build(frm, to, subject, when, text=None, html=None, attach=None, inline=False, extra=(), auth="pass", reply_to=None,
          headers=(), calendar=None, human=False):
    m = EmailMessage()
    m["From"], m["To"], m["Subject"] = frm, to, subject
    if human:
        # what tells the app a person wrote this in a mail program
        m["Message-ID"] = f"<CAF{when}{abs(hash(subject)) % 10000}@mail.gmail.com>"
    for name, value in headers:
        m[name] = value
    if reply_to:
        m["Reply-To"] = reply_to
    m["Date"] = format_datetime(datetime.fromtimestamp(when, timezone.utc))
    m.set_content(text or "See the HTML version.")
    if html:
        m.add_alternative(html, subtype="html")
        if inline:
            m.get_payload()[1].add_related(PNG, "image", "png", cid="<chart1@fake>")
    if calendar:
        m.add_alternative(calendar, subtype="calendar", params={"method": "REQUEST"})
    if attach:
        m.add_attachment(attach[1], maintype="application", subtype="pdf", filename=attach[0])
    for name, data, (main, sub) in extra:
        m.add_attachment(data, maintype=main, subtype=sub, filename=name)
    return auth_results(frm, to, auth) + m.as_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")


def mailbox(owner):
    out, n = [], 0

    def add(thrid, hours_ago, **kw):
        nonlocal n
        n += 1
        flags = {k: kw.pop(k) for k in ("inbox", "seen", "starred", "draft") if k in kw}
        when = NOW - int(hours_ago * 3600)
        kw.setdefault("to", owner)
        if owner in kw["frm"]:
            kw["auth"] = None   # mail the owner sent was never checked on the way in
        out.append(Msg(n, thrid, build(when=when, **kw), when, **flags))

    if owner.startswith("work"):
        add(1, 0.3, frm="Priya Raman <priya@example.com>", subject="Q4 planning doc", human=True,
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
    add(1, 50, frm="Aoife Byrne <aoife@example.com>", subject="Dinner on Saturday?", inbox=False, seen=True, human=True,
        text="Are you two free on Saturday? Thinking of that place in Ginza.")
    add(1, 49, frm=f"Me <{owner}>", to="Aoife Byrne <aoife@example.com>", subject="Re: Dinner on Saturday?",
        inbox=False, seen=True, text="Yes! 7pm works. I'll book.")
    add(1, 3, frm="Aoife Byrne <aoife@example.com>", subject="Re: Dinner on Saturday?", seen=True, human=True,
        text="Perfect. Can you make it for five? Niamh is in town.")
    add(1, 0.2, frm="Niamh Byrne <niamh@example.com>", subject="Re: Dinner on Saturday?", human=True,
        html="<div>See you all there &#127843;</div><blockquote>Perfect. Can you make it for five?</blockquote>")
    add(2, 1, frm="Matt Levine <noreply@news.example.com>", subject="Money Stuff: Fake Newsletter Edition",
        headers=[("List-Unsubscribe", "<https://news.example.com/unsubscribe/abc123>, <mailto:leave@news.example.com?subject=unsubscribe>"),
                 ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click"), ("List-ID", "<moneystuff.news.example.com>")],
        html="""<html><head><style>.h{font:700 28px Georgia,serif}</style></head><body style="background:#ffffff">
        <table width="600" align="center" cellpadding="0" cellspacing="0"><tr><td>
        <div class="h">Money Stuff</div><p style="color:#333">Everything is <b>securities fraud</b>, again.</p>
        <p><img src="cid:chart1@fake" width="64" height="32" alt="chart"></p>
        <p><img src="https://dummyimage.com/560x90/5b2ee8/ffffff.png&text=A+picture+from+the+sender" width="560" height="90" alt="banner"></p>
        <img src="https://tracker.invalid/open.gif" width="1" height="1">
        <script>document.title = "pwned"</script>
        <p><a href="https://example.com/article" onclick="steal()">Read on the web</a></p>
        </td></tr></table></body></html>""", inline=True)
    add(3, 6, frm="=?utf-8?b?44Os44K044K544Kv44O844Or?= <lego@example.jp>", subject="来週のレッスンについて",
        text="来週土曜日のレッスンは14:05からです。", starred=True)
    add(4, 26, frm="Billing <billing@example.com>", subject="Your invoice for October", seen=True,
        html="<p>Your invoice is attached.</p>", attach=("invoice-2026-10.pdf", b"%PDF-1.4 fake invoice"),
        extra=[("receipt.png", png(3000, 4000, (60, 140, 220)), ("image", "png")),
               ("items.csv", b"item,qty,price\nwidget,2,19.99\n<script>alert(1)</script>,1,0\n", ("text", "csv")),
               ("archive.zip", b"PK\x03\x04 not really", ("application", "zip"))])
    # Sender trust: one Gmail could not verify (with a link that lies about where it goes),
    # one that only the bulk mailer vouches for.
    add(5, 2, frm='"service@paypa1.example" <service@paypa1.example>', subject="Your account has been limited",
        auth="fail", reply_to="Account Review <review@collect.example>",
        html="""<p>We noticed unusual activity. Confirm your details within 24 hours:</p>
        <p><a href="https://login.collect.example/verify?id=81">https://www.paypal.com/signin</a></p>
        <p>Or visit <a href="https://example.com/help">our help page</a>.</p>""")
    add(6, 4, frm="Corner Bakery <hello@cornerbakery.example>", subject="This week's bread", auth="via",
        text="Sourdough on Thursday, focaccia on Friday. https://cornerbakery.example/menu")
    # Things a message says about itself: a Google Group passing on someone's mail, an
    # invitation, a flight booking described for mail programs.
    add(7, 1.5, frm="\"'Claire Byrne' via Kids\" <kids@family.example>", subject="School photos on Friday",
        reply_to="Claire Byrne <claire@webmail.example>", text="Remember the uniforms! Photos are at 9.",
        headers=[("X-Original-Sender", "claire@webmail.example"), ("X-Original-From", "Claire Byrne <claire@webmail.example>"),
                 ("X-Original-Authentication-Results", "mx.google.com;       dkim=pass header.i=@webmail.example header.s=s1;       "
                  "spf=pass (google.com: domain of claire@webmail.example designates 192.0.2.7 as permitted sender) "
                  "smtp.mailfrom=claire@webmail.example;       dmarc=pass (p=NONE sp=NONE dis=NONE) header.from=webmail.example"),
                 ("Mailing-list", "list kids@family.example; contact kids+owners@family.example"), ("X-Google-Group-Id", "1234567"),
                 ("List-ID", "<kids.family.example>"), ("List-Post", "<mailto:kids@family.example>"), ("Precedence", "list"),
                 ("List-Unsubscribe", "<mailto:googlegroups-manage+1234567+unsubscribe@googlegroups.com>")])
    add(8, 2.5, frm="Aoife Byrne <aoife@example.com>", subject="Invitation: Hanami picnic", human=True,
        text="You have been invited to Hanami picnic.", calendar="\r\n".join([
            "BEGIN:VCALENDAR", "PRODID:-//Fake//EN", "VERSION:2.0", "METHOD:REQUEST", "BEGIN:VEVENT",
            "DTSTART;TZID=Europe/Dublin:20270403T110000", "DTEND;TZID=Europe/Dublin:20270403T140000",
            "ORGANIZER;CN=Aoife Byrne:mailto:aoife@example.com", "UID:hanami-1@fake",
            f"ATTENDEE;PARTSTAT=NEEDS-ACTION;CN=Me:mailto:{owner}", "ATTENDEE;PARTSTAT=ACCEPTED;CN=Niamh Byrne:mailto:niamh@example.com",
            "ATTENDEE;PARTSTAT=DECLINED:mailto:conor@example.com",
            "SUMMARY:Hanami picnic", "LOCATION:Yoyogi Park\\, west gate", "END:VEVENT", "END:VCALENDAR", ""]))
    add(9, 7, frm="Example Air <bookings@air.example>", subject="Your trip to Dublin is confirmed", html="""
        <div itemscope itemtype="http://schema.org/FlightReservation"><meta itemprop="reservationNumber" content="K7QF2M"/>
        <div itemprop="underName" itemscope itemtype="http://schema.org/Person"><meta itemprop="name" content="Dan Byrne"/></div>
        <div itemprop="reservationFor" itemscope itemtype="http://schema.org/Flight"><meta itemprop="flightNumber" content="42"/>
        <div itemprop="airline" itemscope itemtype="http://schema.org/Airline"><meta itemprop="name" content="Example Air"/><meta itemprop="iataCode" content="EX"/></div>
        <div itemprop="departureAirport" itemscope itemtype="http://schema.org/Airport"><meta itemprop="name" content="Tokyo Haneda"/><meta itemprop="iataCode" content="HND"/></div>
        <meta itemprop="departureTime" content="2026-12-20T10:40:00+09:00"/>
        <div itemprop="arrivalAirport" itemscope itemtype="http://schema.org/Airport"><meta itemprop="name" content="Dublin"/><meta itemprop="iataCode" content="DUB"/></div>
        <meta itemprop="arrivalTime" content="2026-12-20T17:55:00+00:00"/></div>
        <div itemprop="potentialAction" itemscope itemtype="http://schema.org/ViewAction">
        <link itemprop="target" href="https://air.example/trips/K7QF2M"/><meta itemprop="name" content="View trip"/></div></div>
        <p>Thanks for booking with Example Air. Your confirmation code is <b>K7QF2M</b>.</p>""")
    # A conversation whose subject is long enough to wrap in the reading pane.
    long = "Notification of completion of application and of the registration of a contact address for the ticket office lottery, with the details of what happens next and when"
    for i, h in enumerate((30, 29, 28)):
        add(11, h, frm="Tickets <noreply@tickets.example>", subject=long, seen=True, text=f"Message {i + 1} of three.\n" + "A line of text.\n" * 30)
    # A receipt laid out as a table: the price column must not be squeezed to one character.
    add(10, 9, frm="Castle Hire <receipts@pay.example>", subject="Your receipt [#1267-1958]", seen=True, html="""
        <table width="600" align="center"><tr><td style="padding:24px;background:#f6f9fc">
        <table width="100%"><tr><td style="font-size:16px;line-height:24px">Booking Online Payment for #13030: Castle Hire - Asset
        Booking 13030 - Payment for the afternoon of the party, including delivery and collection</td>
        <td width="8"></td><td align="right" valign="top" style="font-size:16px" id="price">&euro;185.00</td></tr></table>
        </td></tr></table>""")
    # A long read, several screens tall, for trying the remembered reading position.
    add(12, 11, frm="Works in Print <essays@longreads.example>", subject="The long essay: how the canal was dug", seen=True,
        html="".join(f"<h2>Part {i + 1}</h2><p>" + f"Paragraph {i + 1} of the essay goes on at some length. " * 30 + "</p>" for i in range(40)))
    # Mail that is only in All Mail: archived conversations (FAKE_ARCHIVE sets how many), one of
    # them unread, and a draft, which the app should not list.
    for i in range(int(os.environ.get("FAKE_ARCHIVE", "40"))):
        add(5000 + i, 60 + i * 11, frm=f"Old Friend {i} <friend{i}@example.org>", subject=f"Archived conversation {i}",
            text=f"This is archived message {i}, kept only in All Mail. " * 3, inbox=False, seen=i != 3, human=True)
    add(5000, 58, frm=f"Me <{owner}>", to="Old Friend 0 <friend0@example.org>", subject="Re: Archived conversation 0",
        inbox=False, seen=True, text="My reply, also archived.")
    add(4999, 2, frm=f"Me <{owner}>", to="nobody@example.org", subject="An unfinished draft", inbox=False, seen=True,
        draft=True, text="This draft should not be listed.")
    # FAKE_ROUTINE=3000 makes a long inbox, for trying the list at a realistic size
    for i in range(int(os.environ.get("FAKE_ROUTINE", "70"))):
        add(100 + i, 30 + i * 7, frm=f"Sender {i} <sender{i}@example.com>", subject=f"Routine message number {i}",
            text=f"This is the body of routine message {i}. " * 4, seen=i % 3 != 0)
    return out


BOXES = {}
VERSION = {}  # user -> counter bumped whenever that mailbox changes; IDLE connections watch it


def touch(user):
    VERSION[user] = VERSION.get(user, 0) + 1

DRAFTS = {}   # user -> messages saved with APPEND


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
            if cmd == "IDLE":
                # hold the connection; say so when this user's mailbox changes; end on DONE
                self.send("+ idling\r\n")
                self.wfile.flush()
                seen = VERSION.get(user, 0)
                while not select.select([self.connection], [], [], 0.1)[0]:
                    if VERSION.get(user, 0) != seen:
                        seen = VERSION.get(user, 0)
                        self.send(f"* {len(self.visible(user, selected))} EXISTS\r\n")
                        self.wfile.flush()
                if not self.rfile.readline():
                    return
                self.send(f"{tag} OK IDLE terminated (Success)\r\n")
                continue
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
                              '* LIST (\\Drafts \\HasNoChildren) "/" "[Gmail]/Drafts"\r\n'
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
                    msgid = re.search(r"X-GM-MSGID (\d+)", args)
                    if thrids:
                        hits = [u for u, x in msgs.items() if x.thrid in thrids]
                    elif msgid:
                        hits = [u for u, x in msgs.items() if x.msgid == msgid.group(1)]
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
                    touch(user)
                    self.send(f"{tag} OK Success\r\n")
                elif cmd == "APPEND":
                    # APPEND "mailbox" (flags) {n}  then the message itself
                    size = int(re.search(r"\{(\d+)\}$", args).group(1))
                    self.send("+ go ahead\r\n")
                    self.wfile.flush()
                    message = self.rfile.read(size)
                    self.rfile.readline()
                    DRAFTS.setdefault(user, []).append(message)
                    self.send(f"{tag} OK [APPENDUID 9 {len(DRAFTS[user])}] (Success)\r\n")
                elif cmd == "UID COPY":
                    # on Gmail, copying into INBOX just adds the Inbox label
                    setstr, _, dest = args.partition(" ")
                    msgs = self.visible(user, selected)
                    if dest.strip('"').upper() == "INBOX":
                        for uid in parse_set(setstr, list(msgs)):
                            msgs[uid].inbox = True
                            msgs[uid].uid_inbox += 5000   # back in the inbox under a new UID
                        touch(user)
                        self.send(f"{tag} OK (Success)\r\n")
                    else:
                        self.send(f"{tag} NO [TRYCREATE] (Failure)\r\n")
                elif cmd == "UID MOVE":
                    setstr, _, dest = args.partition(" ")
                    msgs = self.visible(user, selected)
                    if selected == "inbox" and "All Mail" in dest:
                        for uid in parse_set(setstr, list(msgs)):
                            msgs[uid].inbox = False
                        touch(user)
                        self.send(f"{tag} OK [COPYUID 7 {setstr} {setstr}] (Success)\r\n")
                    else:
                        self.send(f"{tag} NO [TRYCREATE] No folder {dest} (Failure)\r\n")
                elif cmd == "XDELIVER":
                    # test hook: new mail arrives in this user's inbox.  XDELIVER <subject>
                    msgs = box(user)
                    n = max(m.uid_all for m in msgs) + 1
                    raw = build(frm="Postie <postie@example.com>", to=user, subject=args or "Just arrived",
                                when=int(time.time()), text="This one was delivered while you were watching.")
                    msgs.append(Msg(n, 9000 + n, raw, int(time.time())))
                    touch(user)
                    self.send(f"{tag} OK delivered\r\n")
                elif cmd == "XREPLY":
                    # test hook: the owner replies from another mail program to the newest
                    # conversation in the inbox, so the reply is in All Mail only.  XREPLY
                    msgs = box(user)
                    latest = max((m for m in msgs if m.inbox), key=lambda m: m.when)
                    n = max(m.uid_all for m in msgs) + 1
                    subject = latest.raw.split(b"Subject: ", 1)[-1].split(b"\n", 1)[0].decode().strip()
                    raw = build(frm=f"Me <{user}>", to="someone@example.com", subject=re.sub(r"^(Re: )?", "Re: ", subject),
                                when=int(time.time()), text="Sent from my phone while the app was open.", auth=None)
                    msgs.append(Msg(n, int(latest.thrid) - 1800000000000000000, raw, int(time.time()), inbox=False, seen=True))
                    touch(user)
                    self.send(f"{tag} OK replied\r\n")
                elif cmd == "XDRAFTS":
                    # test hook: hand back the newest saved draft
                    saved = DRAFTS.get(user, [])
                    body = saved[-1] if saved else b""
                    self.send(b"* XDRAFTS %d {%d}\r\n" % (len(saved), len(body)) + body + b"\r\n")
                    self.send(f"{tag} OK done\r\n")
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
