import { ConfirmDialog } from './ConfirmDialog';
import { useConfirmStore } from './confirmStore';

/** Shows the question `confirmAction` asked. */
export function ConfirmHost() {
  const { options, settle } = useConfirmStore();
  return <ConfirmDialog open={options !== null} options={options} onConfirm={() => { settle(true); }} onCancel={() => { settle(false); }} />;
}
