import "../design/handoff/tokens.css";
import "../src/styles/app.css";
import { setAvatar } from "../src/avatar.ts";
import { speciesOf } from "../src/office/actors.ts";
import { loadOwls } from "../src/office/sprites.ts";
import { openEmployeeDetail } from "../src/popup-employee.ts";
import { mountPanel, ROLE_LABEL } from "../src/sidebar-right.ts";
import { createStore } from "../src/store.ts";
import { emp, office, team } from "../src/test-office.ts";
import { rankLabel } from "../src/ticker.ts";
import type { Invoke } from "../src/scale.ts";
import type {
  EmployeeDto,
  EmployeeDetailDto,
  OfficeDto,
  OfficeListDto,
  Rank,
  Role,
} from "../src/types/ipc.ts";

const ranks: Rank[] = ["Bujang", "Gwajang", "Daeri", "Sawon"];
const products = [
  ["Claude", "B"],
  ["Codex", "D"],
  ["Gemini", "F"],
] as const;
const role = document.getElementById("role") as HTMLSelectElement;
for (const [value, label] of Object.entries(ROLE_LABEL)) role.add(new Option(label, value));
const gallery = document.getElementById("gallery")!;
const status = document.getElementById("selected")!;
const store = createStore<OfficeListDto, OfficeDto>();
let seq = 1;
let selected: { employee: EmployeeDto; building: string; product: string };
const samples: {
  avatar: HTMLElement;
  employee: EmployeeDto;
  building: string;
  button: HTMLButtonElement;
}[] = [];
const owls = await loadOwls();
const ids = new Map<string, string>();
for (let i = 0; ids.size < owls.meta.employeeSpecies.length; i++) {
  const id = `synthetic-avatar-${i}`;
  ids.set(speciesOf(id, owls.meta.employeeSpecies), id);
}

function update(): void {
  const t = team("preview", [selected.employee], { building: selected.building, deskSlot: 0 });
  const list = store.state.list;
  if (list)
    store.applyList({
      ...list,
      listSeq: ++seq,
      offices: list.offices.map((o) => ({ ...o, building: selected.building })),
    });
  store.applyOfficeState(office("preview", ++seq, [t]));
  status.textContent = `${selected.product} · ${selected.employee.name} ${rankLabel(selected.employee.rank)} · ${ROLE_LABEL[selected.employee.role]}`;
  for (const sample of samples)
    sample.button.setAttribute("aria-pressed", String(sample.employee === selected.employee));
}

for (const [product, building] of products) {
  const section = document.createElement("section");
  const title = document.createElement("h2");
  title.textContent = product;
  const grid = document.createElement("div");
  grid.className = "avatars";
  for (const rank of ranks) {
    for (const species of owls.meta.employeeSpecies) {
      const employee = emp(ids.get(species)!, {
        rank,
        role: "Backend",
        name: species,
        hiredAt: new Date(Date.now() - 600_000).toISOString(),
      });
      const button = document.createElement("button");
      button.className = "sample";
      const avatar = document.createElement("span");
      avatar.className = "ow-avatar";
      avatar.setAttribute("aria-hidden", "true");
      const label = document.createElement("small");
      label.textContent = `${employee.name} ${rankLabel(rank)}`;
      button.append(avatar, label);
      button.addEventListener("click", () => {
        selected = { employee, building, product };
        update();
      });
      grid.append(button);
      samples.push({ avatar, employee, building, button });
      selected ??= { employee, building, product };
    }
  }
  section.append(title, grid);
  gallery.append(section);
}
store.applyWorld({
  listSeq: 1,
  rebuilt: false,
  scale: "Large",
  selectedId: "preview",
  selectRequest: 0,
  offices: [
    {
      officeId: "preview",
      building: "B",
      floor: 1,
      room: 101,
      label: "합성 아바타",
      phase: "Working",
      dot: "Working",
      asking: false,
      border: "Plain",
      delayed: false,
      teams: [],
    },
  ],
  selected: office("preview", seq, []),
});
const invoke: Invoke = async <T>(cmd: string): Promise<T> => {
  if (cmd === "get_employee_timeline") return [] as T;
  if (cmd !== "get_employee_detail") throw new Error(`미리보기 커맨드 없음: ${cmd}`);
  const e = selected.employee;
  const detail: EmployeeDetailDto = {
    agentId: e.agentId,
    name: e.name,
    rank: e.rank,
    role: e.role,
    model: e.model,
    hiredAt: e.hiredAt,
    retiredAt: null,
    bossName: null,
    instruction: "합성 미리보기입니다.",
    filesRead: [],
    filesEdited: [],
    commands: [],
  };
  return detail as T;
};
function showDetail(opener?: HTMLElement): void {
  openEmployeeDetail(
    { invoke, live: () => selected.employee },
    {
      officeId: "preview",
      teamId: "preview",
      agentId: selected.employee.agentId,
      officeName: "합성 미리보기",
      building: selected.building,
    },
    opener,
  );
}
mountPanel(document.getElementById("panel")!, store, {
  viewport: { size: () => ({ w: 1280, h: 800 }), subscribe: () => () => {} },
  onEmployee: (_team, _agent, opener) => showDetail(opener),
});
role.addEventListener("change", () => {
  for (const sample of samples) {
    sample.employee.role = role.value as Role;
    setAvatar(sample.avatar, sample.employee, sample.building);
  }
  update();
});
document
  .getElementById("detail")!
  .addEventListener("click", (e) => showDetail(e.currentTarget as HTMLElement));
for (const sample of samples) setAvatar(sample.avatar, sample.employee, sample.building);
update();
