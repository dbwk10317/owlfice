// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vite-plus/test";
import { Container } from "pixi.js";
import { createStore } from "../store.ts";
import { emp, office as makeOffice, team } from "../test-office.ts";
import type { OfficeDto, OfficeListDto } from "../types/ipc.ts";
import { mountOffice } from "./renderer.ts";

const ticks = vi.hoisted(() => [] as ((t: { deltaMS: number }) => void)[]);
const actors = vi.hoisted(
  () => [] as { onBossPick?: (team: string, x: number, y: number) => void }[],
);
const resets = vi.hoisted(() => [] as ReturnType<typeof vi.fn>[]);
vi.mock("pixi.js", async (original) => {
  const actual = await original<typeof import("pixi.js")>();
  return {
    ...actual,
    Application: class {
      canvas = document.createElement("canvas");
      stage = new actual.Container();
      ticker = {
        add: (fn: (t: { deltaMS: number }) => void) => ticks.push(fn),
        start() {},
        stop() {},
      };
      renderer = { width: 520, height: 320, resize() {} };
      async init() {}
    },
  };
});
vi.mock("./sprites.ts", async (original) => ({
  ...(await original<typeof import("./sprites.ts")>()),
  loadOfficeAssets: async () => ({
    floorplan: {
      grid: { cols: 0, rows: 0 },
      rooms: {},
      lightTints: { Off: "#ffffff" },
      layouts: { large: { placements: [] }, small: { placements: [] } },
    },
    sheet: {},
  }),
  loadOwls: async () => null,
  placeSprite: () => new Container(),
}));
vi.mock("./props.ts", async (original) => ({
  ...(await original<typeof import("./props.ts")>()),
  loadWallFont: async () => {},
  PropsLayer: class {
    reset = vi.fn();
    constructor() {
      resets.push(this.reset);
    }
    update() {}
    tick() {}
  },
}));
vi.mock("./actor-layout.ts", () => ({ seatProps: () => new Set() }));
vi.mock("./actors.ts", () => ({
  ActorLayer: class {
    onBossPick?: (team: string, x: number, y: number) => void;
    constructor() {
      actors.push(this);
    }
    attach() {}
    sync() {}
    update() {}
    placeOverlay() {}
    focusedTeam() {
      return null;
    }
    highlightTeam() {}
    isTyping() {
      return false;
    }
  },
}));

afterEach(() => {
  ticks.length = 0;
  actors.length = 0;
  resets.length = 0;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

test("목록만 바뀌면 rebuilt 장면의 툴팁·소품을 유지하고 새 rebuild는 닫는다", async () => {
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {} }));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
    },
  );
  vi.stubGlobal("innerWidth", 1280);
  vi.stubGlobal("devicePixelRatio", 1);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  const stage = document.body.appendChild(document.createElement("div"));
  vi.spyOn(stage, "getBoundingClientRect").mockReturnValue({ width: 1040, height: 640 } as DOMRect);
  const person = team("t", [emp("boss")]);
  person.props.bossVisit = "회의 중";
  const first: OfficeDto = { ...makeOffice("o", 1, [person]), rebuilt: true };
  const list: OfficeListDto = {
    listSeq: 1,
    rebuilt: false,
    scale: "Large",
    offices: [
      {
        officeId: "o",
        building: "A",
        floor: 1,
        room: 101,
        label: "A동 101",
        phase: "Working",
        dot: "Working",
        asking: false,
        border: "Plain",
        delayed: false,
        teams: [],
      },
    ],
    selectedId: "o",
    selectRequest: 0,
  };
  const store = createStore<OfficeListDto, OfficeDto>();
  store.applyWorld({ ...list, selected: first });
  await mountOffice(stage, store);
  const tick = () => ticks.forEach((fn) => fn({ deltaMS: 16 }));
  tick();
  expect(resets[0]).toHaveBeenCalledTimes(1);
  actors[0].onBossPick?.("t", 20, 20);
  const tooltip = stage.querySelector<HTMLElement>(".ow-canvas-tooltip")!;
  expect(tooltip.hidden).toBe(false);
  store.applyList({ ...list, listSeq: 2 });
  tick();
  expect(resets[0]).toHaveBeenCalledTimes(1);
  expect(tooltip.hidden).toBe(false);
  store.applyOfficeState({ ...first, stateSeq: 2 });
  tick();
  expect(resets[0]).toHaveBeenCalledTimes(2);
  expect(tooltip.hidden).toBe(true);
});
