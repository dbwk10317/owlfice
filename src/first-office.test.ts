import { expect, test } from "vite-plus/test";
import { firstOffice } from "./first-office.ts";
import type { OfficeSummaryDto } from "./types/ipc.ts";

const o = (officeId: string, phase: "Off" | "Working") => ({ officeId, phase }) as OfficeSummaryDto;

test("퇴근한 사무실보다 불 켜진 사무실을 먼저 고르고, 모두 퇴근이면 맨 앞", () => {
  expect(firstOffice([o("a", "Off"), o("b", "Working"), o("c", "Working")])?.officeId).toBe("b");
  expect(firstOffice([o("a", "Off"), o("b", "Off")])?.officeId).toBe("a");
  expect(firstOffice([])).toBeUndefined();
});
