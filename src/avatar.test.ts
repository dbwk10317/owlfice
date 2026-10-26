// @vitest-environment jsdom
import { Rectangle, Texture } from "pixi.js";
import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import raw from "../design/sprites/employees.json";
import { speciesOf, type OwlMeta, type Owls } from "./office/actors.ts";
import { loadOwls } from "./office/sprites.ts";
import { emp } from "./test-office.ts";

vi.mock("./office/sprites.ts", async (orig) => ({
  ...(await orig<typeof import("./office/sprites.ts")>()),
  loadOwls: vi.fn(),
}));

const draws = new Map<HTMLCanvasElement, ReturnType<typeof vi.fn>>();
const textures = Object.fromEntries(
  Object.entries(raw.frames).map(([key, { frame }]) => [
    key,
    new Texture({
      frame: new Rectangle(frame.x, frame.y, frame.w, frame.h),
    }),
  ]),
);
const owls: Owls = { textures, meta: raw.owlfice as unknown as OwlMeta };
const settle = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};
const host = () => {
  const el = document.createElement("span");
  document.body.append(el);
  return el;
};
const copied = (el: HTMLElement): HTMLCanvasElement =>
  draws.get(el.firstChild as HTMLCanvasElement)!.mock.calls[0][0] as HTMLCanvasElement;
/** 그 자리에 그린 몸 겹의 시트 위치가 이 종의 idle_f_0 인가 */
const bodyAt = (el: HTMLElement, species: string) => {
  const f = textures[`body/${species}/idle_f_0`].frame;
  return draws.get(copied(el))!.mock.calls[0].slice(1, 3).join() === [f.x, f.y + 4].join();
};

beforeEach(() => {
  vi.resetModules();
  vi.mocked(loadOwls).mockReset().mockResolvedValue(owls);
  draws.clear();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(function (
    this: HTMLCanvasElement,
  ) {
    const drawImage = vi.fn();
    draws.set(this, drawImage);
    return { drawImage } as unknown as ReturnType<HTMLCanvasElement["getContext"]>;
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

test("몸·직급·담당을 위 네 줄 잘라 가운데 합성하며 같은 외형은 캐시한다", async () => {
  const { setAvatar } = await import("./avatar.ts");
  const a = host();
  const b = host();
  const employee = emp("a", { rank: "Gwajang", role: "Qa" });
  setAvatar(a, employee, "D");
  setAvatar(b, employee, "D");
  await settle();
  const source = copied(a);
  expect(copied(b)).toBe(source);
  expect([source.width, source.height]).toEqual([40, 44]);
  const keys = [
    `body/${speciesOf("a", owls.meta.employeeSpecies)}/idle_f_0`,
    "outfit/gwajang-codex/idle_f_0",
    "role/qa/idle_f_0",
  ];
  expect(draws.get(source)!.mock.calls).toEqual(
    keys.map((key) => {
      const f = textures[key].frame;
      return [textures[key].source.resource, f.x, f.y + 4, 32, 44, 4, 0, 32, 44];
    }),
  );
  expect(a.style.getPropertyValue("--ow-avatar-bg")).toBe("var(--ow-codex-tint)");
  const old = a.firstChild;
  setAvatar(a, employee, "C");
  await settle();
  expect(a.firstChild).toBe(old);
  expect(loadOwls).toHaveBeenCalledTimes(1);
});

test("종·직급·제품·담당이 바뀌면 새 그림이며 수습은 담당 겹이 없다", async () => {
  const { setAvatar } = await import("./avatar.ts");
  const a = host();
  const first = emp("a");
  setAvatar(a, first, "A");
  await settle();
  const sources = [copied(a)];
  const different = ["b", "c", "d", "e", "f"].find(
    (id) => speciesOf(id, owls.meta.employeeSpecies) !== speciesOf("a", owls.meta.employeeSpecies),
  )!;
  for (const [employee, building] of [
    [emp(different), "A"],
    [emp(different, { rank: "Bujang" }), "A"],
    [emp(different, { rank: "Bujang" }), "F"],
    [emp(different, { rank: "Bujang", role: "Trainee" }), "F"],
  ] as const) {
    setAvatar(a, employee, building);
    await settle();
    sources.push(copied(a));
  }
  expect(new Set(sources).size).toBe(5);
  expect(draws.get(sources[sources.length - 1])!.mock.calls).toHaveLength(2);
});

test("로딩 중 직원 교체와 닫힌 팝업에는 옛 그림을 붙이지 않는다", async () => {
  let resolve!: (value: Owls) => void;
  vi.mocked(loadOwls).mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const { setAvatar } = await import("./avatar.ts");
  const a = host();
  const closed = host();
  setAvatar(a, emp("old"), "A");
  setAvatar(a, emp("new", { rank: "Bujang", role: "Writer" }), "E");
  setAvatar(closed, emp("old"), "A");
  closed.remove();
  resolve(owls);
  await settle();
  expect(a.children).toHaveLength(1);
  const source = copied(a);
  const frame = textures["outfit/bujang-gemini/idle_f_0"].frame;
  expect(draws.get(source)!.mock.calls[1].slice(1, 3)).toEqual([frame.x, frame.y + 4]);
  expect(closed.children).toHaveLength(0);
});

test.each(["load", "canvas", "frame"])(
  "자원 실패 %s는 자리표시를 남기고 오류 한 번만 기록한다",
  async (failure) => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    if (failure === "load") vi.mocked(loadOwls).mockRejectedValue(new Error("atlas failed"));
    if (failure === "canvas")
      vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    if (failure === "frame") vi.mocked(loadOwls).mockResolvedValue({ ...owls, textures: {} });
    const { setAvatar } = await import("./avatar.ts");
    const a = host();
    const b = host();
    setAvatar(a, emp("a"), "A");
    setAvatar(b, emp("b"), "B");
    await settle();
    expect(a.children).toHaveLength(0);
    expect(b.children).toHaveLength(0);
    expect(log).toHaveBeenCalledTimes(1);
  },
);

test.each(["Security", "Data", "Designer"] as const)(
  "새 담당 %s는 40×44 카드의 같은 자르기 안에 그린다",
  async (role) => {
    const { setAvatar } = await import("./avatar.ts");
    const a = host();
    setAvatar(a, emp("new", { role, items: ["Helmet", "Blanket"] }), "A");
    await settle();
    const source = copied(a);
    const calls = draws.get(source)!.mock.calls;
    expect([source.width, source.height]).toEqual([40, 44]);
    expect(calls).toHaveLength(3);
    const f = textures[`role/${role.toLowerCase()}/idle_f_0`].frame;
    expect(calls[2]).toEqual([
      textures[`role/${role.toLowerCase()}/idle_f_0`].source.resource,
      f.x,
      f.y + 4,
      32,
      44,
      4,
      0,
      32,
      44,
    ]);
  },
);

test("동물을 바꾸면 시트는 다시 읽지 않고 그 동물의 종으로 다시 그리며 같은 동물이면 캐시를 유지한다", async () => {
  const { setAvatar, setAvatarAnimal } = await import("./avatar.ts");
  const a = host();
  const employee = emp("a");
  setAvatar(a, employee, "A");
  await settle();
  expect(loadOwls).toHaveBeenLastCalledWith(2);
  expect(bodyAt(a, speciesOf("a", owls.meta.employeeSpecies))).toBe(true);
  setAvatarAnimal("mixed");
  setAvatar(host(), employee, "A");
  await settle();
  const old = a.firstChild;
  setAvatarAnimal("dogs");
  setAvatar(a, employee, "A");
  await settle();
  expect(loadOwls).toHaveBeenCalledTimes(1);
  expect(a.firstChild).not.toBe(old);
  expect(a.children).toHaveLength(1);
  expect(bodyAt(a, speciesOf("a", owls.meta.animals.dogs.employees))).toBe(true);
});

test("동물이 바뀌면 붙어 있는 아바타를 다시 그리고 떨어진 자리는 잊는다", async () => {
  const { setAvatar, setAvatarAnimal } = await import("./avatar.ts");
  const a = host();
  const gone = host();
  const employee = emp("a");
  setAvatar(a, employee, "A");
  setAvatar(gone, emp("b"), "A");
  await settle();
  const old = a.firstChild;
  const goneOld = gone.firstChild;
  gone.remove();
  setAvatarAnimal("cats");
  await settle();
  expect(loadOwls).toHaveBeenCalledTimes(1);
  expect(a.firstChild).not.toBe(old);
  expect(a.children).toHaveLength(1);
  expect(bodyAt(a, speciesOf("a", owls.meta.animals.cats.employees))).toBe(true);
  expect(gone.firstChild).toBe(goneOld);
});

test("제거된 요소는 동물을 안 바꿔도 다음 setAvatar 뒤 묶음에서 빠진다", async () => {
  const { setAvatar, mountedCount } = await import("./avatar.ts");
  const a = host();
  const gone = host();
  setAvatar(a, emp("a"), "A");
  setAvatar(gone, emp("b"), "A");
  await settle();
  expect(mountedCount()).toBe(2);
  gone.remove();
  setAvatar(a, emp("c"), "A");
  expect(mountedCount()).toBe(1);
});

test("시트 로드가 한 번 실패해도 다음 setAvatar 가 다시 시도해 그린다", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  vi.mocked(loadOwls).mockRejectedValueOnce(new Error("일시 오류"));
  const { setAvatar } = await import("./avatar.ts");
  const a = host();
  setAvatar(a, emp("a"), "A");
  await settle();
  expect(a.children).toHaveLength(0);
  setAvatar(a, emp("a"), "A");
  await settle();
  expect(loadOwls).toHaveBeenCalledTimes(2);
  expect(a.querySelector("canvas")).not.toBeNull();
  expect(log).toHaveBeenCalledTimes(1);
});
