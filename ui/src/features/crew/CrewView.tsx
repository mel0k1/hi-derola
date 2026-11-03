import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Icon } from "@/lib/icons";
import { Card, EmptyHint, PanelShell, StateBadge } from "@/features/panels/Panels";
import { crewGraphSvg, useStore } from "@/store/app";
import { invoke } from "@/lib/tauri";
import { Md } from "@/lib/md";
import { cn } from "@/lib/utils";
import type { CrewMember, CrewState } from "@/lib/types";

function MemberChip({ m }: { m: CrewMember }) {
  const note = useStore((s) => s.note);
  const refreshCrew = useStore((s) => s.refreshCrew);
  let label = `${m.name} · ${m.role}`;
  if (m.model) label += ` · ${m.model}`;
  if (m.tools) label += " · tools";
  if (m.review) label += " · review";
  if (m.bg) label += " · bg";
  if (m.limit) label += ` · ${m.used || 0}/${m.limit} tok`;
  return (
    <span
      title="click to set a token limit for this member (off clears it)"
      className="flex cursor-pointer items-center gap-1 rounded-md border border-border bg-secondary/60 px-2 py-0.5 font-mono text-[11.5px] text-primary hover:border-primary/50"
      onClick={async () => {
        const v = prompt(`token limit for ${m.name} (e.g. 200000, 200k; off to clear)\nused so far: ${m.used || 0}`, m.limit ? String(m.limit) : "");
        if (v === null) return;
        const t = v.trim().toLowerCase();
        let limit: number | null;
        if (!t || t === "off" || t === "0") {
          limit = null;
        } else {
          const m2 = t.match(/^(\d+(?:\.\d+)?)([km])?$/);
          if (!m2) {
            note(`bad limit "${t}": use 200000, 200k or off`);
            return;
          }
          limit = Math.round(parseFloat(m2[1]) * (m2[2] === "k" ? 1e3 : m2[2] === "m" ? 1e6 : 1));
        }
        try {
          await invoke("crew_limit", { name: m.name, limit });
          await refreshCrew();
        } catch (e) {
          note(String(e));
        }
      }}
    >
      {label}
      <span
        title="re-run this member's turn now (revives an errored or limit-stopped member)"
        className="text-muted-foreground hover:text-primary"
        onClick={async (e) => {
          e.stopPropagation();
          try {
            await invoke("crew_retry", { name: m.name });
            await refreshCrew();
          } catch (err) {
            note(String(err));
          }
        }}
      >
        ↻
      </span>
    </span>
  );
}

function SavedRow({ onOpen, onDel, children }: { onOpen: () => void; onDel?: () => void; children: React.ReactNode }) {
  return (
    <div className="msg-in flex items-center gap-2.5 rounded-lg border border-border bg-secondary/50 px-3 py-2">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">{children}</div>
      <Button variant="ghost" size="xs" onClick={onOpen}>
        resume
      </Button>
      {onDel && (
        <Button variant="ghost" size="xs" className="text-muted-foreground hover:text-destructive" onClick={onDel}>
          del
        </Button>
      )}
    </div>
  );
}

export function CrewView() {
  const crew = useStore((s) => s.crew);
  const st = crew.state;
  const refreshCrew = useStore((s) => s.refreshCrew);
  const refreshCrewSaved = useStore((s) => s.refreshCrewSaved);
  const refreshCrewPresets = useStore((s) => s.refreshCrewPresets);
  const showCrewUsage = useStore((s) => s.showCrewUsage);
  const note = useStore((s) => s.note);
  const pollStart = useStore((s) => s.pollStart);
  const pollStop = useStore((s) => s.pollStop);

  const [formOpen, setFormOpen] = useState(false);
  const [goal, setGoal] = useState("");
  const [members, setMembers] = useState("");
  const [memo, setMemo] = useState("");
  const [crewInput, setCrewInput] = useState("");
  const [addSpec, setAddSpec] = useState("");
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    pollStart("crew");
    return () => pollStop("crew");
  }, [pollStart, pollStop]);

  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [crew.state, crew.live]);

  const roleOf = (name: string) => st?.members?.find((x) => x.name === name)?.role || "";

  const act = async (p: Promise<{ none?: boolean } & Partial<CrewState>>) => {
    try {
      const d = await p;
      useStore.getState().crewApply(d);
    } catch (e) {
      note(String(e));
    }
  };

  return (
    <PanelShell
      icon="crew"
      title="crew"
      sub="several AI agents in one session: they coordinate via @Name, a DONE: line finishes the goal; tokens counted per member, optional tools and token limits"
      action={
        <Button size="sm" onClick={() => setFormOpen(!formOpen)}>
          <Icon name="plus" /> new crew
        </Button>
      }
    >
      {formOpen && (
        <Card className="mb-3">
          <label className="flex flex-col gap-1 text-[13px] text-muted-foreground">
            goal
            <textarea
              rows={2}
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
              placeholder="design and review a release plan for the app"
              className="resize-y rounded-lg border border-input bg-background px-2.5 py-1.5 text-[13px] text-foreground outline-none focus:border-ring"
            />
          </label>
          <label className="mt-2 flex flex-col gap-1 text-[13px] text-muted-foreground">
            members (one per line, Name|role, optional: profile|model|tools|limit=100k; "|role" = the agent picks a name)
            <textarea
              rows={3}
              value={members}
              onChange={(e) => setMembers(e.target.value)}
              placeholder={"Rex|planner|limit=100k\nNora|critic\n|coder|fast|gpt-5-mini|tools"}
              className="resize-y rounded-lg border border-input bg-background px-2.5 py-1.5 font-mono text-[12.5px] text-foreground outline-none focus:border-ring"
            />
          </label>
          <div className="mt-3 flex items-center gap-2">
            <Button
              size="sm"
              onClick={async () => {
                try {
                  await invoke("crew_new", {
                    goal: goal.trim(),
                    members: members.split("\n").map((l) => l.trim()).filter(Boolean),
                  });
                  setFormOpen(false);
                  await refreshCrew();
                } catch (e) {
                  note(String(e));
                }
              }}
            >
              create
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setFormOpen(false)}>
              cancel
            </Button>
          </div>
        </Card>
      )}

      {!st ? (
        <>
          <EmptyHint>no active crew. create one: a goal and a few members. agents talk in rounds, @Name steers the next speaker, a DONE: line finishes the goal.</EmptyHint>
          {crew.savedOpen && (
            <div className="mt-4 flex flex-col gap-2">
              <div className="text-[12px] font-semibold text-muted-foreground">saved crews: click to resume</div>
              {!crew.saved.length ? (
                <div className="text-center text-[13px] text-muted-foreground">no saved crews yet</div>
              ) : (
                crew.saved.map((c) => (
                  <SavedRow
                    key={c.id}
                    onOpen={() => act(invoke("crew_open", { id: c.id }))}
                    onDel={async () => {
                      try {
                        await invoke("crew_drop", { id: c.id });
                        await refreshCrewSaved(true);
                      } catch (e) {
                        note(String(e));
                      }
                    }}
                  >
                    <div className="truncate text-[13px] font-semibold">{c.goal}</div>
                    <div className="truncate text-[11.5px] text-muted-foreground">
                      {c.status} · {c.members} member(s) · {c.messages} msg · {new Date(c.updated * 1000).toLocaleString()}
                      {c.mentions && c.mentions.length ? " · @" + c.mentions.slice(0, 4).map((m) => `${m.from}→${m.to}×${m.n}`).join(", ") : ""}
                    </div>
                  </SavedRow>
                ))
              )}
            </div>
          )}
          {crew.presetsOpen && (
            <div className="mt-5 flex flex-col gap-2">
              <div className="text-[12px] font-semibold text-muted-foreground">crew presets - spawn a fresh crew with the saved members and budget</div>
              {!crew.presets.length ? (
                <div className="text-center text-[13px] text-muted-foreground">no presets yet - save the active crew with the preset button</div>
              ) : (
                crew.presets.map((p) => (
                  <div key={p.name} className="msg-in flex items-center gap-2.5 rounded-lg border border-border bg-secondary/50 px-3 py-2">
                    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <div className="truncate text-[13px] font-semibold">{p.name}</div>
                      <div className="truncate text-[11.5px] text-muted-foreground">
                        {p.members} member(s){p.budget ? " · $" + (+p.budget).toFixed(2) : ""} · {p.goal}
                      </div>
                    </div>
                    <Button variant="ghost" size="xs" onClick={() => act(invoke("crew_spawn", { name: p.name }))}>
                      spawn
                    </Button>
                    <Button
                      variant="ghost"
                      size="xs"
                      onClick={() => {
                        const g = prompt(`new goal for a crew spawned from "${p.name}":`, p.goal);
                        if (g === null) return;
                        act(invoke("crew_spawn", { name: p.name, goal: g.trim() || null }));
                      }}
                    >
                      new goal
                    </Button>
                    <Button
                      variant="ghost"
                      size="xs"
                      className="text-muted-foreground hover:text-destructive"
                      onClick={async () => {
                        try {
                          await invoke("crew_preset_del", { name: p.name });
                          await refreshCrewPresets(true);
                        } catch (e) {
                          note(String(e));
                        }
                      }}
                    >
                      del
                    </Button>
                  </div>
                ))
              )}
            </div>
          )}
        </>
      ) : (
        <>
          <Card className="mb-3">
            <div className="flex flex-wrap items-center gap-2">
              <div className="min-w-0 flex-1 truncate text-[15px] font-semibold">goal: {st.goal}</div>
              <StateBadge state={st.status === "done" ? "done" : st.status === "running" ? "running" : "waiting"} className={cn(st.running && st.status !== "done" && "animate-pulse")} />
              {st.status === "done" && st.running && <StateBadge state="waiting" />}
            </div>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {st.running && (st.bg_running || 0) > 0 && (
                <span className="animate-pulse rounded-md border border-warning/50 bg-warning/5 px-2 py-0.5 font-mono text-[11.5px] text-warning">
                  {st.bg_running} bg running
                </span>
              )}
              {st.budget ? (
                <span className="rounded-md border border-border bg-secondary/60 px-2 py-0.5 font-mono text-[11.5px] text-primary">
                  budget: ${(+(st.spent ?? 0)).toFixed(2)} / $${(+st.budget).toFixed(2)}
                </span>
              ) : null}
              {(st.members || []).map((m) => (
                <MemberChip key={m.name} m={m} />
              ))}
            </div>
          </Card>

          {crew.memoryOpen && (
            <Card className="mb-3">
              <div className="text-[12px] font-semibold text-muted-foreground">
                memory: {(st.memory || []).length} memo(s), injected into every member prompt
              </div>
              {!(st.memory || []).length ? (
                <div className="mt-2 text-[13px] text-muted-foreground">empty. agents add entries with a MEMO: line, you can type one below</div>
              ) : (
                <div className="mt-2 flex flex-col gap-1.5">
                  {(st.memory || []).map((m, i) => (
                    <div key={i} className="flex items-center gap-2.5 rounded-lg border border-border bg-secondary/50 px-3 py-2">
                      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                        <div className="truncate text-[13px] font-semibold">[{i + 1}] {m.author}</div>
                        <div className="text-[11.5px] break-words text-muted-foreground">{m.content}</div>
                      </div>
                      <Button variant="ghost" size="xs" onClick={() => act(invoke("crew_forget", { n: i + 1 }))}>
                        forget
                      </Button>
                    </div>
                  ))}
                </div>
              )}
              <div className="mt-2.5 flex items-center gap-2">
                <input
                  value={memo}
                  onChange={(e) => setMemo(e.target.value)}
                  onKeyDown={async (e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      if (!memo.trim()) return;
                      try {
                        await invoke("crew_memo", { text: memo.trim() });
                        setMemo("");
                        await refreshCrew();
                      } catch (err) {
                        note(String(err));
                      }
                    }
                  }}
                  placeholder="remember: a fact the crew should keep across sessions..."
                  spellCheck={false}
                  className="h-8.5 min-w-0 flex-1 rounded-lg border border-input bg-background px-2.5 text-[13px] outline-none focus:border-ring"
                />
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={async () => {
                    if (!memo.trim()) return;
                    try {
                      await invoke("crew_memo", { text: memo.trim() });
                      setMemo("");
                      await refreshCrew();
                    } catch (err) {
                      note(String(err));
                    }
                  }}
                >
                  memo
                </Button>
              </div>
            </Card>
          )}

          {crew.graphOpen && (
            <Card className="mb-3">
              {crewGraphSvg(st.graph || [], st.members || []) && (
                <div dangerouslySetInnerHTML={{ __html: crewGraphSvg(st.graph || [], st.members || []) || "" }} />
              )}
            </Card>
          )}

          {crew.usage && (
            <Card className="mb-3">
              <div className="text-[13.5px] font-semibold">
                tokens: {crew.usage.rows.length} member(s) · in {crew.usage.total.input} · out {crew.usage.total.output} · ${(+crew.usage.total.cost).toFixed(4)}
                {crew.usage.budget ? ` · budget ${(+(crew.usage.spent || 0)).toFixed(2)}/${(+(crew.usage.budget || 0)).toFixed(2)}` : ""}
              </div>
              {crew.usage.rows.map((r) => (
                <div key={r.name} className="mt-0.5 text-[11.5px] text-muted-foreground">
                  {r.name} ({r.role}): {r.requests} req · in {r.input} · out {r.output} · ${(+r.cost).toFixed(4)}
                </div>
              ))}
            </Card>
          )}

          <div ref={listRef} className="mb-3 flex max-h-[42vh] flex-col gap-2 overflow-y-auto px-0.5 py-1">
            {(st.messages || []).map((m, i) =>
              m.kind === "system" ? (
                <div key={i} className="rounded-lg border border-dashed border-border px-3 py-2 text-[12px] text-muted-foreground italic">
                  {m.content}
                </div>
              ) : (
                <div key={i} className="rounded-lg border border-border bg-secondary/40 px-3 py-2">
                  <div className="mb-0.5 text-[12px] font-semibold text-primary">
                    {m.author}
                    {roleOf(m.author) ? " · " + roleOf(m.author) : ""}
                  </div>
                  <div className="text-[13px] leading-relaxed break-words">
                    <Md text={m.content} />
                  </div>
                </div>
              )
            )}
            {Object.entries(crew.live || {}).map(([key, l]) => (
              <div key={key} className="rounded-lg border border-primary/40 bg-secondary/40 px-3 py-2">
                <div className="mb-0.5 text-[12px] font-semibold text-primary">
                  {l.author} · typing…
                </div>
                <div className="text-[13px] leading-relaxed break-words">
                  <Md text={l.raw} />
                </div>
              </div>
            ))}
          </div>

          <div className="mb-2 flex flex-wrap items-center gap-1.5">
            <Button variant="ghost" size="sm" onClick={() => act(invoke("crew_step", { rounds: 1 }))}>step</Button>
            <Button size="sm" onClick={() => act(invoke("crew_auto", { maxRounds: 8 }))}>auto</Button>
            <Button variant="ghost" size="sm" onClick={() => act(invoke("crew_stop"))}>stop</Button>
            <Button variant="ghost" size="sm" onClick={() => showCrewUsage()}>usage</Button>
            <Button variant="ghost" size="sm" className={cn(crew.graphOpen && "border border-primary/50 text-primary")} title="@mention graph - who steers whom" onClick={() => useStore.setState((s) => ({ crew: { ...s.crew, graphOpen: !s.crew.graphOpen } }))}>
              graph
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="whole-crew $ budget - auto-stops everything once spent"
              onClick={async () => {
                const v = prompt("crew budget in USD - the whole crew stops when this is spent (off clears it)", st.budget ? String(st.budget) : "");
                if (v === null) return;
                const t = v.trim().toLowerCase().replace(/^\$/, "");
                let budget: number | null;
                if (!t || t === "off" || t === "0") budget = null;
                else {
                  budget = parseFloat(t);
                  if (!isFinite(budget) || budget <= 0) {
                    note(`bad budget "${v}" - use 5, 2.50 or off`);
                    return;
                  }
                }
                act(invoke("crew_budget", { budget }));
              }}
            >
              budget
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="toggle a reviewer - a DONE claim needs their APPROVE"
              onClick={async () => {
                const v = prompt("member to (un)mark as reviewer - a DONE claim needs their APPROVE:", "");
                if (!v || !v.trim()) return;
                act(invoke("crew_review", { name: v.trim() }));
              }}
            >
              review
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="re-run one member's turn now - revives an errored or limit-stopped member"
              onClick={async () => {
                const v = prompt("member to re-run now (revives an errored or limit-stopped member):", "");
                if (!v || !v.trim()) return;
                act(invoke("crew_retry", { name: v.trim() }));
              }}
            >
              retry
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="save this crew (goal, members, budget) as a reusable preset"
              onClick={async () => {
                const v = prompt("save this crew as preset (name: letters, digits, - and _):", "");
                if (!v || !v.trim()) return;
                try {
                  const r = await invoke<{ saved: string }>("crew_preset_save", { name: v.trim() });
                  note(`preset ${r.saved} saved - spawn fresh crews from the presets panel`);
                } catch (e) {
                  note(String(e));
                }
              }}
            >
              preset
            </Button>
            <Button variant="ghost" size="sm" className={cn(crew.memoryOpen && "border border-primary/50 text-primary")} title="long-term memory - survives sessions" onClick={() => useStore.setState((s) => ({ crew: { ...s.crew, memoryOpen: !s.crew.memoryOpen } }))}>
              memory
            </Button>
            <Button variant="ghost" size="sm" onClick={() => refreshCrewSaved(true)} title="saved crews - resume or delete">
              saved
            </Button>
            <Button variant="ghost" size="sm" onClick={() => refreshCrewPresets(true)} title="crew presets - spawn a fresh crew with the saved members and budget">
              presets
            </Button>
            <Button variant="ghost" size="sm" onClick={() => act(invoke("crew_drop", { id: st.id }))}>
              delete
            </Button>
          </div>

          <div className="flex items-center gap-2">
            <input
              value={crewInput}
              onChange={(e) => setCrewInput(e.target.value)}
              onKeyDown={async (e) => {
                if (e.key === "Enter" && crewInput.trim()) {
                  try {
                    await invoke("crew_send", { text: crewInput.trim() });
                    setCrewInput("");
                    await refreshCrew();
                    invoke("crew_step", { rounds: 1 }).then(() => refreshCrew()).catch((err) => note(String(err)));
                  } catch (err) {
                    note(String(err));
                  }
                }
              }}
              placeholder="say something to the crew..."
              spellCheck={false}
              className="h-8.5 min-w-0 flex-1 rounded-lg border border-input bg-background px-2.5 text-[13px] outline-none focus:border-ring"
            />
            <Button
              size="sm"
              onClick={async () => {
                if (!crewInput.trim()) return;
                try {
                  await invoke("crew_send", { text: crewInput.trim() });
                  setCrewInput("");
                  await refreshCrew();
                  invoke("crew_step", { rounds: 1 }).then(() => refreshCrew()).catch((err) => note(String(err)));
                } catch (err) {
                  note(String(err));
                }
              }}
            >
              send
            </Button>
          </div>
          <div className="mt-2 flex items-center gap-2">
            <input
              value={addSpec}
              onChange={(e) => setAddSpec(e.target.value)}
              placeholder="add member: Name|role[|profile|model|tools|review|bg|limit=100k]"
              spellCheck={false}
              className="h-8.5 min-w-0 flex-1 rounded-lg border border-input bg-background px-2.5 font-mono text-[12.5px] outline-none focus:border-ring"
            />
            <Button
              variant="ghost"
              size="sm"
              onClick={async () => {
                if (!addSpec.trim()) return;
                try {
                  await invoke("crew_add", { spec: addSpec.trim() });
                  setAddSpec("");
                  await refreshCrew();
                } catch (err) {
                  note(String(err));
                }
              }}
            >
              add
            </Button>
          </div>
        </>
      )}
    </PanelShell>
  );
}
