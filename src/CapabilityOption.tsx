// A control for something this build may lack (#57): disabled, with the reason, when it does.
import { useId } from 'react';
import type { Availability } from './capabilities';

/** A checkbox for something the build may lack: disabled, with the reason, when it does. */
export function CapabilityOption(props: {
  label: string;
  availability: Availability;
  checked: boolean;
  onChange?: (checked: boolean) => void;
}) {
  const id = useId();
  const reasonId = `${id}-reason`;
  const { available, reason } = props.availability;
  return (
    <div className="capability-option">
      <input
        id={id}
        type="checkbox"
        checked={available && props.checked}
        disabled={!available}
        aria-describedby={reason ? reasonId : undefined}
        onChange={(e) => props.onChange?.(e.target.checked)}
      />
      <label htmlFor={id}>{props.label}</label>
      {reason && (
        <p id={reasonId} className="capability-reason">
          {reason}
        </p>
      )}
    </div>
  );
}
