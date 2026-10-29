//! A filled-in snapshot for checking the screens without a server (`KUROOBI_GGS_DEMO`).

use super::*;

pub fn demo_snapshot() -> Snapshot {
    let mut s = Snapshot {
        conn: "online".into(),
        login: "kuroobi".into(),
        ..Default::default()
    };
    s.my_ranks = vec![
        RankRow {
            gtype: "8".into(),
            name: "kuroobi".into(),
            rating: 1842.3,
            dev: 34.0,
            rank: 12,
            wins: 128,
            losses: 74,
            draws: 6,
        },
        RankRow {
            gtype: "8r16".into(),
            name: "kuroobi".into(),
            rating: 1795.0,
            dev: 51.0,
            rank: 27,
            wins: 41,
            losses: 38,
            draws: 2,
        },
    ];
    let mut users: Vec<UserRow> = vec![
        ("saio", 2245.8, 34.0),
        ("tamaki", 2011.4, 46.0),
        ("momo-bot", 2280.1, 22.0),
        ("nara", 1688.2, 91.0),
        ("kei", 1488.9, 216.0),
        ("newbie", 1200.0, 350.0),
    ]
    .into_iter()
    .map(|(n, r, d)| UserRow {
        name: n.into(),
        rating: Some(r),
        dev: Some(d),
        rating_r: Some(r - 30.0),
        dev_r: Some(d + 8.0),
        open: Some(if r > 1700.0 { '+' } else { '-' }),
        raw: format!("{n} {r}@{d}"),
    })
    .collect();
    for i in 0..26 {
        let r = 1900.0 - i as f32 * 21.0;
        users.push(UserRow {
            name: format!("player{:02}", i + 1),
            rating: Some(r),
            dev: Some(40.0 + i as f32),
            rating_r: Some(r - 30.0),
            dev_r: Some(48.0 + i as f32),
            open: Some(if i % 3 == 0 { '-' } else { '+' }),
            raw: format!("player{:02} {r}@40", i + 1),
        });
    }
    s.users = users;
    s.ranking = s.users.clone();
    s.ongoing = vec![
        OngoingView {
            id: ".71.0".into(),
            watching: true,
            names: vec!["tamaki".into(), "momo-bot".into()],
            ratings: vec!["2011.4".into(), "2280.1".into()],
            gtype: "s8r14".into(),
            mine: false,
        },
        OngoingView {
            id: ".72.0".into(),
            watching: false,
            names: vec!["nara".into(), "kei".into()],
            ratings: vec!["1688.2".into(), "1488.9".into()],
            gtype: "8".into(),
            mine: false,
        },
    ];
    s.offers = vec![
        Offer {
            id: "1".into(),
            raw: "+ .1 saio 2245.8 8r16 15:00 R".into(),
            incoming: true,
            names: vec!["saio".into(), "kuroobi".into()],
            gtype: "s8r16".into(),
            time: "00:15:00".into(),
            rated: true,
            since_ms: epoch_ms() - 120_000,
        },
        Offer {
            id: "2".into(),
            raw: "+ .2 tamaki 2011.4 8 10:00".into(),
            incoming: false,
            names: vec!["tamaki".into(), "nara".into()],
            gtype: "8".into(),
            time: "00:10:00".into(),
            rated: false,
            since_ms: epoch_ms() - 45_000,
        },
    ];
    s.stored = vec![StoredView {
        id: "3".into(),
        opp: "tamaki".into(),
        gtype: "s8r16".into(),
    }];
    let cells = |mv: &[(usize, u8)]| {
        let mut c = vec![0u8; 64];
        c[27] = 2;
        c[28] = 1;
        c[35] = 1;
        c[36] = 2;
        for &(i, v) in mv {
            c[i] = v;
        }
        c
    };
    let face = |id: &str, my: &str, turn: &str, mine: u64, opp: u64| MatchView {
        rated: Some(false),
        id: id.into(),
        base: ".71".into(),
        over: false,
        cells: cells(&[(20, 1), (29, 1), (34, 2), (37, 1), (43, 2), (44, 1)]),
        turn: turn.into(),
        my_color: my.into(),
        opp_name: "saio".into(),
        opp_rating: "2245.8".into(),
        my_clock: format!("{}:{:02}", mine / 60, mine % 60),
        opp_clock: format!("{}:{:02}", opp / 60, opp % 60),
        my_secs: Some(mine),
        opp_secs: Some(opp),
        gtype: "s8r16".into(),
        moves: vec!["f5".into(), "d6".into(), "c3".into(), "d3".into()],
        last_eval: Some(2.5),
        last_from_book: true,
        ..Default::default()
    };
    s.matches = vec![
        face(".71.0", "black", "black", 664, 750),
        face(".71.1", "white", "white", 672, 746),
    ];
    s.chat = vec![
        ChatMsg {
            chan: ".chat".into(),
            from: "demo-bob".into(),
            text: "anyone up for a game?".into(),
            at: 1_754_000_000,
            thread: ".chat".into(),
        },
        ChatMsg {
            chan: ".chat".into(),
            from: "kuroobi".into(),
            text: "sure, 15 min?".into(),
            at: 1_754_000_060,
            thread: ".chat".into(),
        },
        ChatMsg {
            chan: ".chat".into(),
            from: "saio".into(),
            text: "good luck both".into(),
            at: 1_754_002_800,
            thread: ".chat".into(),
        },
    ]
    .into();
    s.results = vec![
        (".70", "saio", 6i32, 1_754_003_000u64, 1842.3, "s8r16"),
        (".69", "tamaki", -4, 1_753_990_000, 1836.1, "8"),
        (".68", "nobu", 12, 1_753_900_000, 1840.0, "s8r16"),
        (".67", "kei", 18, 1_753_820_000, 1828.4, "8r16"),
        (".66", "momo-bot", -18, 1_753_740_000, 1812.9, "s8"),
        (".65", "nara", 0, 1_753_650_000, 1825.5, "8"),
    ]
    .into_iter()
    .map(|(id, opp, d, at, rate, gt)| GameResult {
        id: id.into(),
        base: format!("{gt}{id}"),
        raw: format!("{id} {opp} {d:+}"),
        my_diff: Some(d),
        opp: opp.into(),
        at,
        my_rating: Some(rate),
        ..Default::default()
    })
    .collect();
    s.log = vec![
        LogLine {
            seq: 0,
            dir: "info",
            text: "connected (skatgame.net:5000)".into(),
        },
        LogLine {
            seq: 1,
            dir: "out",
            text: "tell /os play .71.0 F5".into(),
        },
        LogLine {
            seq: 2,
            dir: "in",
            text: "/os: match .71.0 update".into(),
        },
        LogLine {
            seq: 3,
            dir: "in",
            text: "/os: | 1 kuroobi 1842.3 11:04 vs saio 2245.8 12:30".into(),
        },
        LogLine {
            seq: 4,
            dir: "out",
            text: "tell /os look".into(),
        },
        LogLine {
            seq: 5,
            dir: "in",
            text: "/os: 12 waiting requests".into(),
        },
    ]
    .into();
    s.standby_stats = StandbyStats {
        games: 12,
        wins: 7,
        losses: 4,
        draws: 1,
        diff_sum: 38,
    };
    let finger = |accept: &str| FingerInfo {
        fields: vec![
            ("open".into(), "1".into()),
            ("rated".into(), "+".into()),
            ("accept".into(), accept.into()),
            ("decline".into(), "rated&or>2400".into()),
            ("play".into(), "0".into()),
            ("stored (+)".into(), "0".into()),
            ("info".into(), "demo data for screen checks".into()),
            ("since".into(), "2026-01-15".into()),
        ],
    };
    s.fingers.insert(
        "kuroobi".into(),
        finger("rand&discs>=14&discs<=20&mt1>=120"),
    );
    s.fingers.insert("saio".into(), finger("rand&or>=1600"));
    s
}
