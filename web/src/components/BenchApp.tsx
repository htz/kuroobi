import React from 'react';
import { useEngine } from '../engine/useEngine';
import { href, t, type Lang } from '../i18n';
import { FFO_SETS, readObf, solvedRight, type ProblemSet } from '../lib/ffo';
import { legal, squareName } from '../lib/othello';
import { Board, BoardDefs, type EvalInfo } from './Board';
import { BenchTable, Busy, Button, Field, Panel, Progress, Segmented, Select, Toast, type BenchRow } from './ui';

/** Each problem is solved to the end from empty tables, as FFO times are taken. */
const SOLVE_ALL = { depth: 1, solve: 64 };

type Got = NonNullable<BenchRow['got']>;

/** The file is read whole in the page; past this a phone's browser would struggle. */
const OBF_MAX_BYTES = 10 << 20;


export default function BenchApp({ lang }: { lang: Lang }) {
  const L = (key: string, vars?: Record<string, string | number>) => t(lang, 'play.' + key, vars);
  const B = (key: string, vars?: Record<string, string | number>) => t(lang, 'bench.' + key, vars);
  const { engine, progress, ready, failed } = useEngine(true);
  const [sets, setSets] = React.useState<ProblemSet[]>(FFO_SETS);
  const [which, setWhich] = React.useState(2);
  const [note, setNote] = React.useState<string | null>(null);
  const picker = React.useRef<HTMLInputElement>(null);
  const [got, setGot] = React.useState<(Got | undefined)[]>([]);
  const [running, setRunning] = React.useState<number | null>(null);
  const [selected, setSelected] = React.useState(0);
  const gen = React.useRef(0);
  const set = sets[which];
  const problem = set.problems[selected];

  const pick = (i: number) => {
    setWhich(i);
    setGot([]);
    setSelected(0);
  };

  const flash = (text: string) => {
    setNote(text);
    setTimeout(() => setNote(null), 2000);
  };
  // A file read in becomes a set of its own, chosen at once; its problems are numbered from 1.
  const load = async (f: File | undefined) => {
    if (!f) return;
    if (f.size > OBF_MAX_BYTES) {
      flash(B('tooBig'));
      return;
    }
    const problems = readObf(await f.text());
    if (!problems.length) {
      flash(B('loadBad'));
      return;
    }
    const added = { name: f.name, label: f.name, problems };
    setSets((all) => [...all, added]);
    pick(sets.length);
  };

  const run = async () => {
    if (!engine) return;
    const g = ++gen.current;
    const out: (Got | undefined)[] = [];
    setGot([]);
    for (let i = 0; i < set.problems.length; i++) {
      const p = set.problems[i];
      setRunning(i);
      setSelected(i);
      await engine.newGame();
      const c = await engine.choose(p.pos.cells, p.pos.turn, SOLVE_ALL);
      if (g !== gen.current) return;
      out[i] = { move: squareName(c.move), score: c.score, right: solvedRight(p, c.move, c.score), ms: c.ms, nodes: c.nodes };
      setGot(out.slice());
    }
    setRunning(null);
  };

  const stop = () => {
    gen.current++;
    engine?.restart();
    setRunning(null);
  };

  const rows: BenchRow[] = set.problems.map((p, i) => ({
    no: p.no, empties: p.empties, got: got[i], running: running === i,
    answer: p.best && p.score !== undefined ? { moves: p.best, score: p.score } : undefined,
  }));
  const done = got.filter(Boolean).length;
  // The obf's values on the board, the best in bold; no mark, as nothing says how they were found.
  const evals: Record<number, EvalInfo> = Object.fromEntries((problem.values ?? []).map((v) =>
    [v.sq, { score: v.score, best: v.score === problem.score }]));
  const busy = running !== null;

  return (
    <div className="pa">
      <BoardDefs />
      <div className="pa-bar">
        <h1 className="pa-title">Kuroobi for web</h1>
        <Segmented label={L('modes')} items={[
          { label: L('title'), href: href(lang, '/play/'), on: false },
          { label: L('studyTitle'), href: href(lang, '/play/study/'), on: false },
          { label: L('benchTitle'), href: href(lang, '/play/bench/'), on: true },
        ]} />
      </div>

      {note && <Toast>{note}</Toast>}

      {failed && (
        <div role="alert" className="pa-failed">
          <span className="pa-strong">{L('loadFailed')}</span>
          <Button onClick={() => location.reload()} style={{ marginLeft: 'auto' }}>{L('reload')}</Button>
        </div>
      )}

      {!ready && !failed && (
        <div role="status" className="pa-loading">
          <span className="pa-strong">{L('loading')}</span>
          <span className="pa-loading-bar"><Progress value={progress?.total ? progress.loaded / progress.total : 0} label={L('loadingAria')} /></span>
          <span className="pa-mono">
            {progress ? `${(progress.loaded / 1e6).toFixed(1)}${progress.total ? ` / ${(progress.total / 1e6).toFixed(1)}` : ''} MB` : ''}
          </span>
        </div>
      )}

      <div className="pa-cols">
        <div className="pa-left">
          <div className="pa-o-kifu">
            <Panel id="bench-results" title={B('results')}>
              <BenchTable lang={lang} rows={rows} current={selected} onSelect={setSelected} />
            </Panel>
          </div>
        </div>
        <div className="pa-right">
          <div className="pa-o-main">
            <Panel id="bench-run" title={B('panel')} note={ready && engine ? B('threads', { n: engine.threads }) : undefined}>
              <Field label={B('set')}>
                <Select label={B('set')} value={String(which)} disabled={busy} onChange={(v) => pick(+v)}
                  options={sets.map((s, i) => [String(i), s.label] as [string, string])} />
              </Field>
              <Button icon="file" disabled={busy} onClick={() => picker.current?.click()}>{B('loadObf')}</Button>
              <input ref={picker} type="file" hidden
                onChange={(e) => { void load(e.target.files?.[0]); e.target.value = ''; }} />
              {busy ? (
                <div role="status" className="pa-analysis">
                  <div className="pa-analysis-row">
                    <Busy>{B('running')}</Busy>
                    <span className="pa-mono">{done} / {set.problems.length}</span>
                    <Button onClick={stop} style={{ marginLeft: 'auto' }}>{B('stop')}</Button>
                  </div>
                  <Progress value={done / set.problems.length} label={B('panel')} />
                </div>
              ) : (
                <Button kind="primary" disabled={!ready} onClick={run}>{B('run')}</Button>
              )}
            </Panel>
          </div>
          <div className="pa-o-board pa-bench-board">
            <figure className="pa-figure">
              <div className={'pa-board' + (ready ? '' : ' pa-dim')}>
                <Board lang={lang} cells={problem.pos.cells} legal={legal(problem.pos.cells, problem.pos.turn)}
                  evals={evals} turn={problem.pos.turn === 1 ? 'b' : 'w'} last={null} coords disabled onPlay={() => {}} />
              </div>
              <figcaption className="pa-meta">
                {B('caption', { set: set.name, no: problem.no, e: problem.empties, side: L(problem.pos.turn === 1 ? 'black' : 'white') })}
              </figcaption>
            </figure>
          </div>
        </div>
      </div>
    </div>
  );
}
