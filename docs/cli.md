# CLI ツール

学習・計測・対戦に使うコマンド群。`cargo run --release --bin <名前> -- <引数>`
で動く (以下は `target/release/` のバイナリを直接呼ぶ書き方)。

引数の解析は自前で、**`--help` を実装しているのは `arena` だけ**。他は引数
なしで呼ぶと使い方を表示するか、そのまま既定値で走り出す (ベンチ系は後者)。

| 用途 | コマンド |
|---|---|
| 学習 | [`train`](#train) [`nnue_train`](#nnue_train) [`selfplay`](#selfplay) |
| 棋力の比較 | [`arena`](#arena) [`nnue_arena`](#nnue_arena) [`lab`](#lab) [`roundrobin`](#roundrobin) |
| 精度の計測 | [`valmse`](#valmse) [`phase_mse`](#phase_mse) [`wstats`](#wstats) |
| 速度の計測 | [`solve_obf`](#solve_obf) [`flipbench`](#flipbench) [`mpbench`](#mpbench) [`nnue_bench`](#nnue_bench) |
| データ・定石 | [`kifu2data`](#kifu2data) [`bookgen`](#bookgen) [`mpccalib`](#mpccalib) [`nnue_symmetrize`](#nnue_symmetrize) |
| オンライン対戦 | [`ggs`](#ggs) |

---

## 学習

### train

パターン評価 (線形) の教師あり学習。**入りきらないデータはシャードに分けて**
学習する — ファイル単位で `--max-examples` までまとめ、1 エポックごとに各
シャードを読んでは捨てる。

```sh
train [OPTIONS] <data-file>...
```

入力は `.data` (17 バイト固定長) か `.txt` (`<64 文字の盤面> <石差>` を 1 行に
1 件)。どちらかは拡張子で決まる。

| オプション | 既定 | 意味 |
|---|---|---|
| `--epochs <n>` | 10 | 全データを何周するか |
| `--lr <f>` | 0.01 | Adam の学習率 |
| `--weights <path>` | `weights.bin` | 読み込み先 (あれば) と保存先。**毎エポック保存する** |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` |
| `--limit <n>` | 全件 | ファイルごとに使う件数の上限 |
| `--max-examples <n>` | 64M | 同時に RAM へ載せる件数 (`0` = 全部) |
| `--log <path>` | — | エポックごとのステージ別損失を CSV で追記 |
| `--optimizer <k>` | `sgd` | `sgd` / `adam`。**`--lr` の意味が変わる**ので既定を変えるときは学習率も見直す |
| `--swa` | — | 重みの移動平均を取る (Stochastic Weight Averaging) |
| `--swa-start <n>` | 2 | 平均を取り始めるエポック |

```sh
train --epochs 20 --lr 0.008 --weights weights/linear.bin train_data/*.data
```

### nnue_train

NNUE (1 隠れ層) の学習。`train` と同じ 17 バイト形式を読む。**毎エポック、
凍結した重みで検証 MSE を測って表示する** — 学習 MSE ではなくこちらが、
線形評価と比べるべき数字。

```sh
nnue_train [OPTIONS] <data-file>...
```

| オプション | 意味 |
|---|---|
| `--epochs <n>` | 周回数 |
| `--lr <f>` | SGD の学習率 |
| `--decay <f>` | 学習率の減衰 |
| `--threads <n>` | 学習の並列数 |
| `--limit <n>` | 使う件数の上限 |
| `--val <file>` | 検証集合 (複数回渡せる) |
| `--val-cap <n>` | 検証に使う件数の上限 |
| `--out <path>` | 保存先。**最良 val の重みは `<out>.best` に別途残る** |
| `--init <path>` | 初期重み (続きから学習する) |
| `--max-examples <n>` | 同時に RAM へ載せる件数 |

```sh
nnue_train --epochs 30 --lr 0.002 --val val.data \
           --out weights/nnue-h16.bin train_data/*.data
```

### selfplay

自己対戦による強化学習。1 手読みの貪欲 + ε 乱択で指し、終盤はソルバで厳密に
決着させ、TD(λ) で重みを更新する。

```sh
selfplay [OPTIONS]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--games <n>` | 10000 | 自己対戦の局数 |
| `--weights <path>` | `weights.bin` | 読み込み + 更新先 |
| `--lr <f>` | 0.0005 | SGD の学習率 |
| `--decay <f>` | 1.0 | `--save-every` ごとに学習率へ掛ける |
| `--lambda <f>` | 0.7 | TD(λ)。1.0 = モンテカルロ、0.0 = TD(0) |
| `--epsilon <f>` | 0.10 | 一様乱択で指す確率 (探索の多様性) |
| `--solve-empties <n>` | 12 | この空きマス数から完全読み (`0` で無効) |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` |
| `--save-every <n>` | 500 | 何局ごとに保存するか |
| `--opponents <a,b,…>` | — | 相手の重みをカンマ区切りで複数。指定すると自己対戦ではなく総当たりになる |

---

## 棋力の比較

### arena

**2 つの重みを直接対戦させる。**同じ開局を先後入れ替えて 2 局ずつ指し、A の
勝率を 95% 信頼区間つきで出す。**`--help` を持つ唯一のツール。**

```sh
arena --a <weights-A> --b <weights-B> [OPTIONS]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--games <n>` | 1000 | 総局数 (偶数へ切り上げ) |
| `--random-plies <n>` | 6 | 開局をランダムに指す手数 |
| `--depth <n>` | 1 | 両者の中盤探索の深さ (`1` = 貪欲) |
| `--solve-empties <n>` | 0 | 両者がこの空きマス数から完全読み (`0` = 無効) |
| `--depth-a` / `--depth-b` | `--depth` | 片側だけの深さ |
| `--solve-a` / `--solve-b` | `--solve-empties` | 片側だけの読切 |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` / `egaroucid-plus` |
| `--patterns-a` / `--patterns-b` | `--patterns` | 片側だけのパターン |
| `--seed <n>` | 7 | 乱数の種 |
| `--mpc-a` / `--mpc-b` | — | 片側だけ確率的枝刈り (ProbCut) を入れる |
| `--mpc-t <f>` | 1.1 | ProbCut の閾値 (σ の倍数)。小さいほどよく刈る |

**片側だけを指定できるのが要点。**深さ・読切・パターンを非対称にすると、
どの条件差が効いたのかを分けて測れる。

```sh
# 実戦条件で 400 局
arena --a weights/linear.bin --b weights/exp/new.bin \
      --games 400 --depth 8 --solve-empties 12
```

### nnue_arena

**NNUE と線形評価を、同じ探索・同じ深さで戦わせる。**探索の手間が等しいので
差は評価関数だけに出る (速度は無関係 — NNUE は毎ノード再計算する)。

```sh
nnue_arena --nnue <nnue.bin> --linear <weights.bin> [OPTIONS]
```

| オプション | 意味 |
|---|---|
| `--depth <n>` | 両者の固定深さ |
| `--games <n>` | 総局数 |
| `--random-plies <n>` | 開局をランダムに指す手数 |
| `--seed <n>` | 乱数の種 |

### lab

**外部エンジンとの直接対戦。**3 つの方言に対応する。

| `--protocol` | 相手 | やり取り |
|---|---|---|
| `edax` (既定) | Edax のコンソール | `setboard <盤面>` / `go` → `Edax plays XX` |
| `zebra` | Zebra の思考エンジン | `setboard` / `go` → `move xx` |
| `egaroucid` | GTP | 盤面を送らず着手列を再生する (GTP に局面指定が無いため) |

対局の進行はこちらが持ち、相手には局面と `go` だけを渡す。**パスや着手の
反響を同期させる必要がない。**

```sh
lab --edax <path-to-edax-binary> [OPTIONS]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--weights <path>` | `weights/linear.bin` | こちらの重み |
| `--patterns <set>` | `egaroucid` | パターンライブラリ |
| `--depth <n>` | 6 | こちらの中盤の深さ |
| `--solve-empties <n>` | 14 | こちらの読切 |
| `--edax-level <n>` | 5 | 相手のレベル / 深さ |
| `--protocol <p>` | `edax` | 上の表 |
| `--threads <n>` | 1 | 両者のスレッド数。**こちらは終盤しか並列化しないので相手に有利** |
| `--games <n>` | 200 | 総局数 (偶数へ切り上げ) |
| `--random-plies <n>` | 6 | 開局をランダムに指す手数 |
| `--seed <n>` | 7 | 乱数の種 |
| `--per-game` | — | 1 局 1 行で出す (`game <組> <B\|W> <石差>`)。同じ種の 2 回の実行を対にして比べられる |

### roundrobin

**複数エンジンの総当たり。**同じ開局集合を全組が戦い、先後も入れ替える。
評価関数を比べるのが目的なので、全エンジンを素の固定深さに揃える (N 手読み
なら空き N 以下は自然に読み切るので、終盤の設定は要らない)。

```sh
roundrobin --games <n> --depth <n> [--engine name=protocol=path]...
```

`protocol` は `edax` / `zebra` / `egaroucid` / `ours`。**`ours` はちょうど
1 つ**必要で、そのパスは無視される。

```sh
roundrobin --games 100 --depth 8 \
  --engine kuroobi=ours=- \
  --engine edax=edax=/path/to/edax \
  --engine egaroucid=egaroucid=/path/to/egaroucid
```

### ponderhit

**ポンダリングの予測手がどれくらい当たるかを測る。**予測手 1 本を追う方式の
値打ちはこの的中率でほぼ決まる。**予測は探索し直さず、自分の着手後の局面を
置換表に問い合わせて出す** — 実際のポンダリングもそうするしかないので、
それより良い予測を測っても意味がない。

```sh
ponderhit [OPTIONS]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--games <n>` | 20 | 対局数 |
| `--depth <n>` | 8 | 自分の中盤深さ |
| `--solve-empties <n>` | 14 | 自分の完全読み開始 |
| `--opp-depth <n>` | `--depth` | 相手の中盤深さ |
| `--opp-solve <n>` | `--solve-empties` | 相手の完全読み開始 |
| `--threads <n>` | 1 | スレッド数 |
| `--random-plies <n>` | 8 | 開幕の乱数手数 |
| `--seed <n>` | 7 | 乱数の種 |
| `--nnue` / `--weights` | `weights/nnue-h16.bin` / `weights/linear.bin` | 重み |

**`--opp-depth` で相手だけ弱くできる。**予測が相手の強さにどれだけ依るかを
見るため。実測では大幅に弱くしても的中率は 4 ポイントしか落ちなかった。

```sh
# 同じ強さ同士と、相手だけ弱い場合
ponderhit --games 14 --depth 12 --solve-empties 18
ponderhit --games 12 --depth 12 --solve-empties 18 --opp-depth 4
```

### ponderarena

**ポンダリングの効果を測る。**先読みの有無で 2 回走らせ、同じプレイヤーの
合計どうしを比べる。**A と B を戦わせて両者を比べる形は使えない** — 持つ色も
直面する局面も違うので偏りが乗る (対照実験で 24.5% の差が出たことがある)。

```sh
ponderarena [OPTIONS]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--games <n>` | 10 | 対局数 |
| `--ms <n>` | 200 | 1 手の持ち時間 (ミリ秒) |
| `--ponder <on\|off>` | `on` | 先読みするか。`off` は対照実験 |
| `--fixed-depth` | — | 深さ固定で測る。**見るのは勝率ではなく探索時間** |
| `--ponder-ms <n>` | 300 | 深さ固定のときの先読み時間 |
| `--no-mpc` | — | 確率的枝刈りを切る |
| `--depth <n>` | 20 | 中盤深さの上限 |
| `--solve-empties <n>` | 14 | 完全読み開始 |
| `--threads <n>` | 1 | スレッド数 |
| `--random-plies <n>` | 8 | 開幕の乱数手数 |
| `--seed <n>` | 7 | 乱数の種 |
| `--nnue` / `--weights` | `weights/nnue-h16.bin` / `weights/linear.bin` | 重み |

**深さ固定では 1 スレッドで走らせる。**並列探索 (Lazy SMP) は非決定的で、
同じ条件でも着手が変わって対局が分岐する。1 スレッドなら先読みの有無で
着手は 1 手も変わらないので、棋譜の指紋で突き合わせられる。

1 局ごとに置換表を消す (CLAUDE.md の 4 番)。

```sh
# 深さ固定。同じ種で 2 回走らせて合計を比べる
ponderarena --games 14 --fixed-depth --depth 13 --solve-empties 20 --ponder on
ponderarena --games 14 --fixed-depth --depth 13 --solve-empties 20 --ponder off
```

---

## 精度の計測

### valmse

重みを更新せずに検証集合の MSE を測る。`train` の早期打ち切りを判断する
ためのもので、**ステージ別**に出す。

```sh
valmse [--patterns <set>] <weights.bin> <data-file>...
```

### phase_mse

`valmse` の NNUE 版で、**空きマス数別**に区切って出す (CSV)。深さの梯子を
振る実験がこの軸で動くため。外部の評価関数に同じ局面を採点させるための
テキスト書き出しも持つ。

```sh
phase_mse <nnue.bin> <data-file>...              # 空きマス別 MSE (CSV)
phase_mse --dump-text <out.txt> <data-file>...   # 「盤面 スコア」の行に落とす
```

### wstats

**重みファイルの統計。**ステージ帯 × パターンごとに、非ゼロセルの割合
(SGD/Adam は訪れたセルしか触らないので、非ゼロ = 訪問済み) と、その RMS に
向き数を掛けた「1 局面あたりの寄与の目安」を出す。

```sh
wstats [--patterns egaroucid|edax] <weights.bin>
```

---

## 速度の計測

### solve_obf

**FFO ベンチマーク (OBF 形式) の一括求解。**局面ごとに時間・ノード数・NPS を
出す。`edax -solve <file>` の出力と直接比べられる形式。

```sh
solve_obf [--depth <n>] [--weights <path>] <file.obf>...
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--hash-bits <n>` | 26 | 置換表の大きさ (2^n エントリ) |
| `--mpc-t <f>` | — | ProbCut の閾値。指定すると確率的枝刈りを入れる |

`--depth` を渡すと完全読みではなく**固定深さの中盤探索**になる (同じ局面で
探索速度だけを比べたいとき)。

```sh
solve_obf bench/ffo40-59.obf
```

### flipbench

石返しと合法手生成のマイクロベンチ。引数なし。

**結果は「どこに費用がありそうか」の目印であって、判断材料ではない。**
ランダムなマスは分岐予測を外し、ランダムな局面は fill の連鎖を伸ばすので、
実際の探索より両方向へ悲観的に出る (関数テーブル経由の石返しはここで
14.3 ns、実際の求解では約 2.6 ns)。採否は FFO40-59 で決める。

### mpbench

中盤の並列探索が**逐次と同じ手を返すか**と、どれだけ速くなるかを見る。

```sh
mpbench [depth]     # 既定 12
```

### nnue_bench

線形評価と差分 NNUE の**ノード処理量**を、探索と同じ形の全幅走査で比べる。
同じノード集合を辿るので、実時間の比がそのまま探索に持ち込まれる NPS の
影響になる。差分累積器が一から計算した評価と一致することも確かめる。

```sh
nnue_bench [--nnue <path>] [--depth <n>] [--val <file>]...
```

`--val` を渡すと、f32 の前向き計算と i16 量子化の MSE を比べる。

---

## データ・定石

### kifu2data

**棋譜 (`f5d6…`) を学習データに変換する。**全局面に、その対局の最終石差
(空きマスは勝者に加算) をラベルとして付ける。不正な手を含む対局は飛ばして
数える。出力は `train` が読む 17 バイト形式 (黒 u64 LE、白 u64 LE、スコア
i8。**黒番へ正規化**し、スコアも黒視点)。

```sh
kifu2data [OPTIONS] <transcript>...
```

| オプション | 意味 |
|---|---|
| `--limit-games <n>` | 入力ファイルごとに変換する対局数の上限 |
| `--skip-games <n>` | 先頭 n 局を飛ばす (**学習と重ならない検証集合を切り出す**ため) |
| `--skip-plies <k>` | 各対局の先頭 k 局面を記録しない (開局がランダムなデータでは、その結果ラベルは雑音) |
| `--out <file>` | 全部を 1 ファイルに連結する |
| `--out-dir <dir>` | 入力ごとに 1 つ出す (`<dir>/<入力名>.data`) |

### bookgen

**定石の生成。**2 段階で作る。

```sh
# 1. WTHOR (公式大会棋譜) から序盤の頻出局面を候補として積む (未評価)
bookgen --scan train_data/wthor --max-ply 24 --min-games 3 --out book.txt

# 2. 未評価・浅い評価のエントリを実戦より深い探索で解く
bookgen --deepen book.txt --depth 26 --solve 30 --band 8 [--limit 500]
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--book <path>` | — | `--out` の別名 |
| `--hash-bits <n>` | 19 | 中盤置換表の大きさ (2^n エントリ) |
| `--max-cands <n>` | 4 | 1 局面から広げる候補手の数。増やすと木が太る |

**定石の値は「実戦では届かない深さ」でなければ意味がない**ので、既定は
深さ 26 / 読切 30 / 帯 8 (実戦の GGS 設定は 22 / 26 / 6)。途中で止めても
保存済みの分は残るので、何度でも継ぎ足せる。止めるまで回し続けるループは
`tools/book-loop.sh`。

### mpccalib

**ProbCut の較正データを作る。**局面ごとに、置換表をクリアしながら複数の
深さで独立に探索し、1 局面 1 行の CSV に全深さの値を出す。誤差モデル
σ(空き, 深さ, 浅い深さ) はこのデータから別途あてはめる。

```sh
mpccalib [--patterns <set>] [--stride N] [--max N] <weights.bin> <data-file>...
```

### nnue_symmetrize

**NNUE の重みを 8 対称で平均し、評価を対称不変にする。**パターンのマスクは
並び順が対称変換で変わるため、同じ配置でも別のインデックスを引いて評価が
ずれる (実測 0.1〜0.8 石)。軌道ごとに平均して根治する。

```sh
nnue_symmetrize <in.bin> <out.bin> [--val <file>]
```

`--val` を渡すと、対称化の前後で検証 MSE を測って表示する (品質確認)。

---

## オンライン対戦

### ggs

**GGS (skatgame.net:5000) のクライアント。** リバーシのサービス `/os` で非
レートの 8×8 を指す。GUI の「GGS」画面が同じ機能を持つので、普段はそちらで
足りる。

```sh
# 対局する
ggs --play <相手> [--games N]
    [--login 名 --pw パス | --credentials .ggs_credentials]
    [--type 8] [--time 30:00] [--resume <対局 id>]
    [--depth N] [--solve-empties N] [--selective-band N] [--mpc]
    [--solver-hash 22] [--threads N] [--weights path] [--nnue path]

# 着手だけを返すブリッジ (stdin で「<64 面> <X|O>」を受け「= <座標>」を返す)
ggs --serve
```

| オプション | 既定 | 意味 |
|---|---|---|
| `--type <t>` | `8` | 対局形式。`s8r16` (同期・ランダム16手) など。GUI の一覧と同じ記法 |
| `--time <hh:mm>` | `30:00` | 持ち時間 |
| `--resume <id>` | — | 中断対局を再開する |
| `--solver-hash <n>` | 22 | 完全読み用の置換表の大きさ (2^n エントリ) |

認証情報を平文で渡さずに済むよう `--credentials` がある。GUI 側は macOS の
キーチェーンに保存する。
