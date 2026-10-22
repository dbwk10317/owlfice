// 개발용 소품 상태 비교 페이지 (npm run dev 에서 /dev/props.html). 앱 빌드에 들어가지 않는다.
// 바뀐 소품 상태를 PropsLayer 로 그려 tools/props_ref.py 가 만든 참조와 견준다.
import { Rectangle, type Spritesheet } from "pixi.js";
import {
  CANVAS_H,
  CANVAS_W,
  hexToRgb,
  type Kind,
  type LightName,
  type Theme,
} from "../src/office/floorplan.ts";
import { PropsLayer } from "../src/office/props.ts";
import { buildScene, createOfficeApp } from "../src/office/renderer.ts";
import { loadOfficeAssets } from "../src/office/sprites.ts";
import { emp as testEmp, office as testOffice, team as testTeam } from "../src/test-office.ts";
import type { OfficeDto, TeamDto, TodoItem } from "../src/types/ipc.ts";

import { compare, loadRef, show, unpremultiply } from "./ref.ts";

declare global {
  interface Window {
    __propsDiff?: Record<string, number>;
  }
}

const todo = (status: TodoItem["status"], n: number): TodoItem[] =>
  Array.from({ length: n }, () => ({ content: "x", status }));
const team = (deskSlot: number, plant: number | null, building: string): TeamDto => {
  const t = testTeam(`t${deskSlot}`, [], {
    building,
    daylight: building === "C" ? "night" : "evening",
  });
  t.deskSlot = deskSlot;
  t.props.plant = plant;
  if (deskSlot === 0) {
    const night = building === "C";
    t.employees = [
      testEmp("boss", {
        hiredAt: since(NOW(night ? 23 : 18), night ? 240 : 120),
        uniqueReads: night ? 100 : 50,
        uniqueEdits: 20,
        pendingCount: 2,
      }),
    ];
    t.testsToday = { passed: night ? 105 : 12, failed: night ? 7 : 2 };
    t.commitsToday = 2;
    t.props.pr = [1, "https://example.invalid/pull/1"];
    t.props.night = night;
  }
  return t;
};
const office = (shared: Partial<OfficeDto["shared"]>, teams: TeamDto[]): OfficeDto => {
  const o = testOffice("props-reference", 0, teams);
  return { ...o, shared: { ...o.shared, ...shared } };
};
const NOW = (h: number) => new Date(2026, 9, 21, h, 0).getTime();
const since = (now: number, min: number) => new Date(now - min * 60_000).toISOString();

interface Case {
  ref: string;
  kind: Kind;
  theme: Theme;
  lit: LightName | null;
  office: OfficeDto | null;
  now: number;
}

// tools/props_ref.py 의 CASES 와 같은 상태여야 한다
const CASES: Case[] = [
  {
    ref: "large-claude",
    kind: "large",
    theme: "claude",
    lit: null,
    office: null,
    now: NOW(12),
  },
  {
    ref: "props-large-claude",
    kind: "large",
    theme: "claude",
    lit: null,
    now: NOW(18),
    office: office(
      {
        light: "Green",
        safeSince: since(NOW(18), 37),
        trash: 13,
        pizza: 2,
        night: false,
        stats: { commits: 1, filesEdited: 2, commands: 3, turns: 4 },
        kanban: [...todo("Pending", 2), ...todo("InProgress", 1), ...todo("Done", 2)],
      },
      [team(0, 0.7, "A")],
    ),
  },
  {
    ref: "props-small-codex-night",
    kind: "small",
    theme: "codex",
    lit: "night",
    now: NOW(23),
    office: office(
      {
        light: "Red",
        safeSince: since(NOW(23), 1234),
        trash: 20,
        pizza: 8,
        night: true,
        stats: { commits: 0, filesEdited: 9, commands: 10, turns: 12345 },
        kanban: [...todo("Pending", 1), ...todo("InProgress", 2), ...todo("Done", 1)],
      },
      [team(0, 0.5, "C"), team(1, 0.9, "C"), team(3, 1.5, "C")],
    ),
  },
];
CASES.push({ ...CASES[1], ref: "props-large-claude-evening", lit: "evening" });

/** TV 화면 평행사변형(office.json props.tv.screen). 점 한 픽셀 여유를 둔다. 참조에 TV 선이 없어 견주지 않는다 */
function tvScreenTest(x0: number, y0: number, sheet: Spritesheet) {
  const q = (
    sheet.data as unknown as {
      owlfice: { props: { tv: { screen: Record<string, [number, number]> } } };
    }
  ).owlfice.props.tv.screen;
  return (x: number, y: number): boolean => {
    const dx = x - x0;
    if (dx < q.tr[0] || dx > q.tl[0]) return false;
    const t = (dx - q.tl[0]) / (q.tr[0] - q.tl[0]);
    const top = q.tl[1] + t * (q.tr[1] - q.tl[1]);
    const bottom = q.bl[1] + t * (q.br[1] - q.bl[1]);
    return y - y0 >= top - 1 && y - y0 <= bottom + 1;
  };
}

async function main(): Promise<void> {
  const out = document.getElementById("out")!;
  const app = await createOfficeApp();
  const assets = await loadOfficeAssets();
  const result: Record<string, number> = {};
  for (const c of CASES) {
    const scene = buildScene(assets, c.kind, c.theme, false);
    new PropsLayer(assets.floorplan, assets.sheet, c.kind, scene, null, false).update(
      c.office,
      c.now,
    );
    const tv = assets.floorplan.layouts[c.kind].placements.find((p) => p.id === "tv")!;
    const inTvScreen = tvScreenTest(tv.x, tv.y, assets.sheet);
    if (c.lit) {
      const [r, g, b] = hexToRgb(assets.floorplan.lightTints[c.lit]);
      for (const l of scene.lit) l.tint = (r << 16) | (g << 8) | b;
    }
    app.stage.addChild(scene.root);
    const { pixels } = app.renderer.extract.pixels({
      target: app.stage,
      frame: new Rectangle(0, 0, CANVAS_W, CANVAS_H),
      resolution: 1,
    });
    scene.root.destroy({ children: true });
    const ref = await loadRef(c.ref);
    const { count, diff } = compare(ref, pixels, c.lit ? 1 : 0, inTvScreen);
    result[c.ref] = count;
    const sec = document.createElement("section");
    const h = document.createElement("h2");
    h.textContent = `${c.ref}: 다른 픽셀 ${count}`;
    if (count) h.className = "bad";
    sec.append(h);
    show(sec, ref);
    show(sec, unpremultiply(pixels));
    show(sec, diff);
    out.append(sec);
  }
  window.__propsDiff = result;
}

main().catch((e) => {
  document.getElementById("out")!.textContent = `실패: ${e}`;
  console.error(e);
});
