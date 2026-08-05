# CLI ツール

学習・計測・対戦に使うコマンド群。

[← README に戻る](../README.md)

---

## CLI ツール

| コマンド | 役割 |
|---|---|
| `train` / `nnue_train` | 教師あり学習 (線形 / NNUE、シャード読み込み対応) |
| `selfplay` | 自己対戦による強化学習 |
| `arena` / `nnue_arena` | 2 つの重みの直接対戦 (勝率 + 95% 信頼区間) |
| `lab` | 外部エンジンとの直接対戦 + 計測台 (edax / zebra / GTP プロトコル) |
| `roundrobin` | 複数エンジンの総当たり戦ドライバ |
| `ggs` | GGS (skatgame.net) 対局クライアント (--play / --serve / --resume) |
| `solve_obf` | FFO ベンチマーク (OBF 形式) の一括求解 |
| `valmse` / `phase_mse` | 検証セットに対する MSE (ステージ別 / 空きマス別) |
| `mpccalib` | ProbCut の誤差モデル較正データ生成 |
| `kifu2data` | 棋譜 (`f5d6…`) → 学習データ変換 |
| `wstats` | 重みファイルの統計 (訪問率分析など) |
| `flipbench` / `mpbench` / `nnue_bench` | マイクロベンチマーク |

`arena` は per-side 設定 (`--depth-a/-b`, `--solve-a/-b`, `--patterns-a/-b`,
`--mpc-a/-b`) に対応しており、非対称な条件比較ができる。

---

---

[← README に戻る](../README.md)
