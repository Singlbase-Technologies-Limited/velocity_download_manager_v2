import { useMutation, useQueryClient } from '@tanstack/react-query';
import { FolderOpen } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '../../components/Button';
import { CategoryGlyph } from '../../components/CategoryGlyph';
import { Dialog } from '../../components/Dialog';
import { Field } from '../../components/Field';
import { Input } from '../../components/Input';
import { CategoryIconSchema, type Category, type CategoryIcon } from '../../contract';
import { api, type CategoryInput } from '../../lib/api';
import { cn } from '../../lib/cn';
import { toApiError } from '../../lib/errors';
import { keys } from '../../lib/queries';
import { parseExtensions } from './parseExtensions';

// V1's colours, which match Rust's `#rrggbb` rule.
const COLORS = ['#2563eb', '#0e7490', '#16a34a', '#ca8a04', '#ea580c', '#dc2626', '#c026d3', '#7c3aed', '#64748b'] as const;

/** Creates a category, or edits one (V1's CategoryEditor). Rust checks every value; its refusal is shown here. */
export function CategoryEditor({ category, onClose }: { category: Category | null; onClose: () => void }) {
  const qc = useQueryClient();
  const [name, setName] = useState(category?.name ?? '');
  const [color, setColor] = useState<string>(category?.color ?? COLORS[0]);
  const [icon, setIcon] = useState<CategoryIcon>(category?.icon ?? 'folder');
  const [extensions, setExtensions] = useState(category?.extensions.join(' ') ?? '');
  const [directory, setDirectory] = useState(category?.directory ?? '');
  const [browseError, setBrowseError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (input: CategoryInput) => (category ? api.updateCategory(category.id, input) : api.createCategory(input)),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.categories });
      toast.success(category ? 'Category saved' : 'Category created');
      onClose();
    },
  });

  const browse = async () => {
    setBrowseError(null);
    try {
      const chosen = await api.chooseDirectory(directory.trim() || null);
      if (chosen) setDirectory(chosen);
    } catch (e) {
      setBrowseError(toApiError(e).message);
    }
  };

  const failure = save.isError ? toApiError(save.error).message : browseError;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={category ? `Edit ${category.name}` : 'New category'}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            busy={save.isPending}
            disabled={!name.trim()}
            onClick={() => { save.mutate({ name: name.trim(), color, icon, extensions: parseExtensions(extensions), directory: directory.trim() || null }); }}
          >
            Save
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Name">{(p) => <Input {...p} value={name} maxLength={40} onChange={(e) => { setName(e.target.value); }} autoFocus />}</Field>
        <div>
          <p className="text-xs font-medium text-fg-muted">Colour and icon</p>
          <div className="mt-2 flex flex-wrap gap-1.5" role="radiogroup" aria-label="Colour">
            {COLORS.map((c) => (
              <button
                key={c}
                type="button"
                role="radio"
                aria-checked={color === c}
                aria-label={c}
                onClick={() => { setColor(c); }}
                className={cn('size-6 rounded-full ring-offset-2 ring-offset-surface', color === c && 'ring-2 ring-accent')}
                style={{ backgroundColor: c }}
              />
            ))}
          </div>
          <div className="mt-2 flex flex-wrap gap-1" role="radiogroup" aria-label="Icon">
            {CategoryIconSchema.options.map((i) => (
              <button
                key={i}
                type="button"
                role="radio"
                aria-checked={icon === i}
                aria-label={i}
                onClick={() => { setIcon(i); }}
                className={cn('flex size-8 items-center justify-center rounded-md border border-transparent hover:bg-surface-3', icon === i && 'border-accent bg-accent-soft')}
              >
                <CategoryGlyph icon={i} color={color} />
              </button>
            ))}
          </div>
        </div>
        <Field label="File extensions" hint="Separated by spaces or commas, for example “zip 7z rar”.">
          {(p) => <Input {...p} value={extensions} onChange={(e) => { setExtensions(e.target.value); }} placeholder="zip 7z rar" />}
        </Field>
        <Field label="Save to" hint="Leave it empty for the default folder, in a subfolder named after the category when that setting is on.">
          {(p) => (
            <div className="flex gap-2">
              <Input {...p} value={directory} onChange={(e) => { setDirectory(e.target.value); }} placeholder="Default folder" />
              <Button variant="secondary" onClick={() => void browse()}>
                <FolderOpen aria-hidden /> Browse
              </Button>
            </div>
          )}
        </Field>
        {failure ? (
          <p role="alert" className="text-xs text-danger">
            {failure}
          </p>
        ) : null}
      </div>
    </Dialog>
  );
}
