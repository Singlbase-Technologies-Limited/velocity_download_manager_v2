// A download's connections as a line of its details (#57).
import { connectionSummary } from './connections';
import type { ConnectionDetail } from './contract';

/** The summary as a line of the download details. */
export function ConnectionLine(props: { active: number; detail: ConnectionDetail | null }) {
  return <p className="connection-line">{connectionSummary(props.active, props.detail)}</p>;
}
