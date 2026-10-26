/** 직원 식별 키. 중소기업 사무실은 여러 호스트의 팀을 담아 agentId 만으로는 겹칠 수 있다 (docs/CONTRACTS.md C8) */
export const empKey = (teamId: string, agentId: string): string => `${teamId}\u0000${agentId}`;
