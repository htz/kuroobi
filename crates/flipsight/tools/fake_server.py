#!/usr/bin/env python3
"""A local stand-in for the FlipSight bot API, to try the bot without the real server.

    python3 crates/flipsight/tools/fake_server.py <port> [clock seconds]
    FS_BOT_TOKEN=local flipsight --base http://127.0.0.1:<port>

It offers the bot one standard match (accepted) and one anti match (declined); the opponent
answers every move with a random legal one after 0.2 s and sends an eval with it. The state
goes out on the stream only (bot API v2): agent_poll answers 410.

    FAKE_DROP=<seconds>   closes every stream after that long, to try reconnecting.
    FAKE_DELAY=<seconds>  the opponent's thinking time (0.2).
"""
import json, os, random, sys, threading, time
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

ME, OPP, LIMIT = 7, 99, float(sys.argv[2]) if len(sys.argv) > 2 else 10.0
lock = threading.Lock()
rnd = random.Random(1)
now = lambda: int(time.time() * 1000)
DROP = float(os.environ.get("FAKE_DROP", 0))
DELAY = float(os.environ.get("FAKE_DELAY", 0.2))
# Fresh ids every run: the bot does not write a match it has already written.
FIRST = int(time.time()) % 1000000 * 10
events, polls = [], [0]


def public(m):
    return {k: v for k, v in m.items() if not k.startswith("_")}


def emit(m):
    events.append(json.dumps({"type": "match", "view": "agent", "match": public(m)}))


def legal(board, c):
    out = []
    for i in range(64):
        if board[i] == 0 and flips(board, c, i):
            out.append(i)
    return out


def flips(board, c, m):
    o, r0, f0 = 3 - c, *divmod(m, 8)
    for dr in (-1, 0, 1):
        for df in (-1, 0, 1):
            if dr == df == 0:
                continue
            r, f, n = r0 + dr, f0 + df, 0
            while 0 <= r < 8 and 0 <= f < 8 and board[r * 8 + f] == o:
                r, f, n = r + dr, f + df, n + 1
            if n and 0 <= r < 8 and 0 <= f < 8 and board[r * 8 + f] == c:
                return True
    return False


def _apply(board, c, m):
    b = board[:]
    o, r0, f0 = 3 - c, *divmod(m, 8)
    b[m] = c
    for dr in (-1, 0, 1):
        for df in (-1, 0, 1):
            if dr == df == 0:
                continue
            r, f, line = r0 + dr, f0 + df, []
            while 0 <= r < 8 and 0 <= f < 8 and b[r * 8 + f] == o:
                line.append(r * 8 + f)
                r, f = r + dr, f + df
            if line and 0 <= r < 8 and 0 <= f < 8 and b[r * 8 + f] == c:
                for i in line:
                    b[i] = c
    return b


def new_match(mid, mode="std"):
    hist, c = [], 1
    board = [0] * 64
    board[27] = board[36] = 2
    board[28] = board[35] = 1
    while len(hist) < 8:
        mv = rnd.choice(legal(board, c))
        hist.append({"color": c, "move": mv})
        board = _apply(board, c, mv)
        c = 3 - c
    legs = []
    for bp in (0, 1):
        legs.append({"board": board[:], "turn": c, "black_participant": bp, "placed": {"1": 0, "2": 0},
                     "clocks_ms": [LIMIT * 1000, LIMIT * 1000], "turn_started_ms": 0, "revision": 0,
                     "history": [], "status": "active", "discs": [0, 0], "finished_ms": None, "legal_moves": []})
    st = {"version": 1, "score_policy": "ggs_empties_v1", "game_mode": mode, "time_limit_sec": LIMIT,
          "status": "active", "revision": 0, "opening_history": hist, "opening_moves": [h["move"] for h in hist],
          "legs": legs, "totals": [0, 0], "winner": None, "finish_reason": None, "board_size": 8,
          "blocked_squares": []}
    return {"id": mid, "bot_a": OPP, "bot_b": ME, "game_mode": mode, "time_limit_sec": LIMIT,
            "status": "pending", "state": None, "_state": st, "expires_ms": now() + 120000}


matches = {FIRST + 1: new_match(FIRST + 1), FIRST + 2: new_match(FIRST + 2, "anti")}


def refresh(leg):
    leg["discs"] = [leg["board"].count(1), leg["board"].count(2)]
    leg["legal_moves"] = legal(leg["board"], leg["turn"]) if leg["status"] == "active" else []


def play(m, li, mv, ev=None):
    leg = m["state"]["legs"][li]
    t = now()
    leg["clocks_ms"][leg["turn"] - 1] -= t - leg["turn_started_ms"]
    if leg["clocks_ms"][leg["turn"] - 1] < 0:
        leg["status"] = "timeout"
        loser = leg["turn"]
        leg["result_discs"] = [0, 64] if loser == 1 else [64, 0]
        print("TIMEOUT leg", li, "color", loser, flush=True)
    else:
        c = leg["turn"]
        if mv >= 0:
            leg["board"] = _apply(leg["board"], c, mv)
        leg["history"].append({"color": c, "move": mv, "at_ms": t, **({} if ev is None else {"eval": ev})})
        leg["turn"] = 3 - c
        if not legal(leg["board"], 1) and not legal(leg["board"], 2):
            leg["status"] = "finished"
    leg["turn_started_ms"] = t
    leg["revision"] += 1
    refresh(leg)
    finish(m)


def finish(m):
    s = m["state"]
    s["revision"] += 1
    if all(l["status"] != "active" for l in s["legs"]):
        tot = [0, 0]
        for l in s["legs"]:
            d = l.get("result_discs") or l["discs"][:]
            if "result_discs" not in l:
                e = 64 - sum(d)
                if d[0] > d[1]: d[0] += e
                elif d[1] > d[0]: d[1] += e
            bp = l["black_participant"]
            tot[bp] += d[0]
            tot[1 - bp] += d[1]
        s["totals"] = tot
        s["winner"] = None if tot[0] == tot[1] else (0 if tot[0] > tot[1] else 1)
        s["status"] = "finished"
        s["finish_reason"] = "discs"
        m["status"] = "finished"
        m["record"] = {"format": 1, "kind": "bot", "id": m["id"], "updated_ms": now(), "legs": [],
                       "opponent": {"kind": "bot", "bot_id": OPP, "name": "random", "rating_before": 1500.0}}
        print("FINISHED", m["id"], tot, flush=True)
    emit(m)


def opponent_loop():
    while True:
        time.sleep(DELAY)
        with lock:
            for m in matches.values():
                if m["status"] != "running":
                    continue
                for li, leg in enumerate(m["state"]["legs"]):
                    oc = 1 if leg["black_participant"] == 0 else 2
                    if leg["status"] == "active" and leg["turn"] == oc:
                        if leg["legal_moves"]:
                            play(m, li, rnd.choice(leg["legal_moves"]), rnd.randint(-8, 8))
                        else:
                            play(m, li, -1)


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, code, obj):
        obj["server_now_ms"] = now()
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def line(self, obj):
        if isinstance(obj, str):
            obj = json.loads(obj)
        obj["server_now_ms"] = now()
        self.wfile.write((json.dumps(obj) + "\n").encode())
        self.wfile.flush()

    def do_GET(self):
        if "action=state" in self.path:
            print("ROSTER", flush=True)
            bots = [{"id": ME, "name": "local", "online": True, "games_now": 0, "rating": {"rating": 1500.0},
                     "supported_games": ["std"], "max_games": 2},
                    {"id": OPP, "name": "random", "online": True, "is_builtin": True, "rating": {"rating": 1200.0},
                     "games_now": sum(m["status"] == "running" for m in matches.values()), "supported_games": ["std"]}]
            return self.reply(200, {"ok": True, "bots": bots})
        self.send_response(200)
        self.end_headers()
        print("STREAM", flush=True)
        with lock:
            at = len(events)
            live = [public(m) for m in matches.values() if m["status"] in ("pending", "running")]
        self.line({"type": "hello", "bot_id": ME, "state_follows": True, "api": 2})
        self.line({"type": "state", "view": "agent", "ok": True, "scope": "active",
                   "bot": {"id": ME, "name": "local", "max_games": 2}, "matches": live})
        began = last = time.time()
        while not DROP or time.time() - began < DROP:
            time.sleep(0.02)
            for e in events[at:]:
                self.line(e)
                at += 1
            if time.time() - last > 10:
                last = time.time()
                self.line({"type": "ping"})

    def do_POST(self):
        assert self.headers["Authorization"].startswith("Bearer ")
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        a = req["action"]
        with lock:
            if a == "agent_poll":
                polls[0] += 1
                print("POLL", polls[0], flush=True)
                return self.reply(410, {"ok": False, "error": "agent_poll was removed; use bot-stream"})
            if a == "agent_games":
                recs = [m["record"] for m in matches.values()
                        if "record" in m and m["record"]["updated_ms"] >= req.get("since", 0)]
                body = "".join(json.dumps(r) + "\n" for r in recs).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/x-ndjson")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if a == "agent_environment":
                print("ENVIRONMENT", json.dumps(req["environment"]), flush=True)
                return self.reply(200, {"ok": True})
            m = matches.get(req.get("match_id"))
            if a == "agent_accept":
                m["status"], m["state"] = "running", m["_state"]
                for l in m["state"]["legs"]:
                    l["turn_started_ms"] = now()
                    refresh(l)
                print("ACCEPT", m["id"], flush=True)
                emit(m)
                return self.reply(200, {"ok": True, "match": {"id": m["id"], "status": "running"}})
            if a == "agent_reject":
                m["status"] = "rejected"
                print("REJECT", m["id"], flush=True)
                emit(m)
                return self.reply(200, {"ok": True, "match": {"id": m["id"], "status": "rejected"}})
            if a == "agent_move":
                leg = m["state"]["legs"][req["leg"]]
                mc = 1 if leg["black_participant"] == 1 else 2
                mv = req["move"]
                if leg["revision"] != req["revision"]:
                    return self.reply(409, {"ok": False, "error": "stale_revision"})
                if leg["turn"] != mc:
                    return self.reply(409, {"ok": False, "error": "not_your_turn"})
                if (mv == -1 and leg["legal_moves"]) or (mv != -1 and mv not in leg["legal_moves"]):
                    print("ILLEGAL", req, flush=True)
                    return self.reply(409, {"ok": False, "error": "illegal_move"})
                play(m, req["leg"], mv, req.get("eval"))
                return self.reply(200, {"ok": True, "match": {"id": m["id"], "status": m["status"]}})
        self.reply(400, {"ok": False, "error": "unknown action"})


threading.Thread(target=opponent_loop, daemon=True).start()
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
