// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import { mountOffice } from "./renderer.ts";
import { loadOwls } from "./sprites.ts";

// WebGL·에셋 로딩만 대신하고 실제 mountOffice의 배율·패닝과 DOM을 검증한다.
vi.mock("pixi.js", async (original) => {
  const actual = await original<typeof import("pixi.js")>();
  return {
    ...actual,
    Application: class {
      canvas = document.createElement("canvas");
      stage = { scale: { set() {} } };
      ticker = { add() {}, start() {}, stop() {} };
      renderer = {
        width: 520,
        height: 320,
        resize: (w: number, h: number) => {
          this.renderer.width = this.canvas.width = w;
          this.renderer.height = this.canvas.height = h;
        },
      };
      async init() {}
    },
  };
});
vi.mock("./sprites.ts", async (original) => ({
  ...(await original<typeof import("./sprites.ts")>()),
  loadOfficeAssets: async () => ({ floorplan: {}, sheet: {} }),
  loadOwls: vi.fn(async () => null),
}));
vi.mock("./props.ts", async (original) => ({
  ...(await original<typeof import("./props.ts")>()),
  loadWallFont: async () => {},
}));
const layers = vi.hoisted(() => [] as { args: unknown[]; setAnimal: ReturnType<typeof vi.fn> }[]);
vi.mock("./actors.ts", () => ({
  ActorLayer: class {
    setAnimal = vi.fn();
    constructor(...args: unknown[]) {
      layers.push({ args, setAnimal: this.setAnimal });
    }
    placeOverlay() {}
  },
}));

let resize: ResizeObserverCallback;
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("innerWidth", 1280);
  vi.stubGlobal("devicePixelRatio", 1);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {} }));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: ResizeObserverCallback) {
        resize = cb;
      }
      observe() {}
    },
  );
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

async function setup(w: number, h: number) {
  const stage = document.createElement("div");
  document.body.append(stage);
  vi.spyOn(stage, "getBoundingClientRect").mockReturnValue({ width: w, height: h } as DOMRect);
  const onScale = vi.fn();
  const handle = await mountOffice(
    stage,
    {
      state: { list: null, office: null, selectedId: null },
      subscribe: () => () => {},
    },
    { onScale },
  );
  const wrap = stage.querySelector<HTMLElement>(".ow-canvas-wrap")!;
  return {
    handle,
    onScale,
    stage,
    wrap,
    resize(w: number, h: number) {
      resize(
        [
          { devicePixelContentBoxSize: [{ inlineSize: w, blockSize: h }] },
        ] as unknown as ResizeObserverEntry[],
        {} as ResizeObserver,
      );
      vi.advanceTimersByTime(100);
    },
  };
}

test("최초 자동맞춤 뒤 선택 배율은 줄어든 창에 맞추고 커지면 복귀하며 한도 변화도 알린다", async () => {
  // 창이 더 커도 자동 배율은 3 까지다 (HANDOFF 2.2, 배율 메뉴 1·2·3)
  const s = await setup(2080, 1280);
  expect(s.onScale).toHaveBeenLastCalledWith(3, 3);
  s.handle.setScale(2);
  expect(s.onScale).toHaveBeenLastCalledWith(2, 3);
  s.resize(520, 320);
  expect(s.onScale).toHaveBeenLastCalledWith(1, 1);
  expect(s.wrap.style.width).toBe("520px");
  s.resize(1560, 960);
  expect(s.onScale).toHaveBeenLastCalledWith(2, 3);
  s.handle.setScale(1);
  s.onScale.mockClear();
  s.resize(1040, 640);
  expect(s.onScale).toHaveBeenCalledExactlyOnceWith(1, 2);
  s.onScale.mockClear();
  for (const invalid of [0, 4, 1.5, NaN, Infinity]) s.handle.setScale(invalid);
  expect(s.onScale).not.toHaveBeenCalled();
  s.resize(1560, 960);
  expect(s.onScale).toHaveBeenLastCalledWith(1, 3);
});

test("DPR의 장치 픽셀 한계와 최소모드1×를 지키고 최소모드에서 나온 뒤 선호배율을 복원한다", async () => {
  vi.stubGlobal("devicePixelRatio", 2);
  const s = await setup(800, 500);
  expect(s.onScale).toHaveBeenLastCalledWith(3, 3);
  expect(s.wrap.style.width).toBe("780px");
  s.handle.setScale(2);
  s.resize(1039, 640);
  expect(s.onScale).toHaveBeenLastCalledWith(1, 1);
  s.resize(1040, 640);
  expect(s.onScale).toHaveBeenLastCalledWith(2, 2);
  vi.stubGlobal("innerWidth", 719);
  s.resize(1400, 1000);
  expect(s.onScale).toHaveBeenLastCalledWith(1, 1);
  expect(s.wrap.style.width).toBe("260px");
  vi.stubGlobal("innerWidth", 720);
  s.resize(1440, 1000);
  expect(s.onScale).toHaveBeenLastCalledWith(2, 2);
});

test("1×가 안 들어가는 무대는 패닝하고 가운데로는 이동만 초기화한다", async () => {
  const s = await setup(300, 200);
  expect(s.stage.classList.contains("is-panning")).toBe(true);
  s.wrap.setPointerCapture = () => {};
  const pointer = (type: string, x: number, y: number) => {
    const e = new MouseEvent(type, { clientX: x, clientY: y, buttons: 1 });
    Object.defineProperty(e, "pointerId", { value: 1 });
    s.wrap.dispatchEvent(e);
  };
  pointer("pointerdown", 100, 100);
  pointer("pointermove", 140, 120);
  pointer("pointerup", 140, 120);
  expect(s.wrap.style.transform).toBe("translate(40px, 20px)");
  s.onScale.mockClear();
  s.handle.centerCanvas();
  expect(s.wrap.style.transform).toBe("");
  expect(s.stage.querySelector<HTMLElement>(".ow-minimap")!.hidden).toBe(false);
  expect(s.onScale).not.toHaveBeenCalled();
});

test("동물은 기본 섞기로 액터 층에 넘기고, 바꿔도 시트를 다시 읽지 않고 액터 층의 종만 바꾼다", async () => {
  vi.mocked(loadOwls)
    .mockClear()
    .mockResolvedValueOnce({ textures: {}, meta: {} } as never);
  layers.length = 0;
  const s = await setup(1040, 640);
  expect(loadOwls).toHaveBeenCalledExactlyOnceWith(2);
  expect(layers[0].args[4]).toBe("mixed");
  s.handle.setAnimal("owls");
  s.handle.setAnimal("cats");
  expect(layers[0].setAnimal.mock.calls).toEqual([["owls"], ["cats"]]);
  expect(loadOwls).toHaveBeenCalledTimes(1);
});
