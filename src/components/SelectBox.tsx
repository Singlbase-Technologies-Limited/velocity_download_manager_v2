/** A checkbox that can also be partly checked (`'mixed'`): a list's select-all, or a folder of a torrent (#59, #63). */
export function SelectBox({ checked, label, onChange, disabled }: { checked: boolean | 'mixed'; label: string; onChange: () => void; disabled?: boolean }) {
  return (
    <input
      type="checkbox"
      aria-label={label}
      className="size-3.5 accent-[var(--accent)]"
      checked={checked === true}
      disabled={disabled}
      ref={(el) => { if (el) el.indeterminate = checked === 'mixed'; }}
      onChange={onChange}
    />
  );
}
