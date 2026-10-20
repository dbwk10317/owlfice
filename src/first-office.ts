import type { OfficeSummaryDto } from "./types/ipc.ts";

/** 첫 자동 선택: 불이 켜진 첫 사무실, 없으면 맨 앞 (퇴근한 방이 먼저 골라지지 않게) */
export const firstOffice = (offices: OfficeSummaryDto[]): OfficeSummaryDto | undefined =>
  offices.find((o) => o.phase !== "Off") ?? offices[0];
