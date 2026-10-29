import { Button } from '../../components/Button';
import type { AddRequest } from '../../contract';
import { only, plural, type Outcome } from './outcome';

const item = 'rounded-md bg-surface-2 px-3 py-2';

/** What happened to each link that wasn't simply added. */
export function AddOutcomeView({ outcome }: { outcome: Outcome }) {
  const direct = outcome.conflicts.filter((c) => !c.torrent);
  const torrents = outcome.conflicts.filter((c) => c.torrent);
  const listed = outcome.duplicates.filter((d) => !d.duplicate.canAddAnyway);
  const again = outcome.duplicates.filter((d) => d.duplicate.canAddAnyway);
  return (
    <div className="space-y-4 text-[13px]">
      {outcome.added > 0 ? <p className="text-fg-muted">{outcome.added === 1 ? 'One download was added.' : `${plural(outcome.added, 'download')} were added.`}</p> : null}
      {direct.length > 0 ? (
        <section>
          <p className="font-medium">{direct.length === 1 ? 'A file with this name already exists' : `${String(direct.length)} files already exist`}</p>
          <ul className="mt-2 space-y-1.5 text-xs">
            {direct.map(({ source, check }) => (
              <li key={source} className={item}>
                <p className="truncate font-medium">{check.fileName === null ? check.directory : `${check.directory}/${check.fileName}`}</p>
                <p className="mt-0.5 text-fg-muted">
                  {check.suggestedName ? `Keep both saves it as “${check.suggestedName}”.` : 'No free name is left for it in this folder.'}
                  {check.partialDownloadExists && !check.fileExists ? ' Another download is saving a file with this name.' : ''}
                </p>
              </li>
            ))}
          </ul>
          <p className="mt-2 text-xs text-fg-muted">Velocity never replaces a file that is already there.</p>
        </section>
      ) : null}
      {torrents.length > 0 ? (
        <section>
          <p className="font-medium">{torrents.length === 1 ? 'A torrent’s name is taken in this folder' : `${String(torrents.length)} torrents’ names are taken in this folder`}</p>
          <ul className="mt-2 space-y-1.5 text-xs">
            {torrents.map(({ source, check }) => (
              <li key={source} className={item}>
                <p className="truncate font-medium">{check.fileName === null ? check.directory : `${check.directory}/${check.fileName}`}</p>
              </li>
            ))}
          </ul>
          <p className="mt-2 text-xs text-fg-muted">A torrent keeps the names in its metadata, so it can’t be renamed. Choose another folder to add it.</p>
        </section>
      ) : null}
      {listed.length > 0 ? (
        <section>
          <p className="font-medium">{listed.length === 1 ? 'This torrent is already in your list' : `${String(listed.length)} torrents are already in your list`}</p>
          <ul className="mt-2 space-y-1.5 text-xs">
            {listed.map(({ source, duplicate }) => (
              <li key={source} className={item}>
                <p className="truncate font-medium">{duplicate.name ?? source}</p>
              </li>
            ))}
          </ul>
          <p className="mt-2 text-xs text-fg-muted">
            Velocity can’t run two copies of the same torrent: they would download the same pieces into the same files. Resume the one in your list, or remove it first.
          </p>
        </section>
      ) : null}
      {again.length > 0 ? (
        <section>
          <p className="font-medium">{again.length === 1 ? 'This was added before' : `${String(again.length)} were added before`}</p>
          <ul className="mt-2 space-y-1.5 text-xs">
            {again.map(({ source, duplicate }) => (
              <li key={source} className={item}>
                <p className="truncate">{`${duplicate.name ?? source} · ${duplicate.inList ? 'In your list' : 'In your history'}`}</p>
              </li>
            ))}
          </ul>
          <p className="mt-2 text-xs text-fg-muted">Adding it again starts a separate download.</p>
        </section>
      ) : null}
      {outcome.errors.length > 0 ? (
        <section role="alert">
          <p className="font-medium text-danger">{outcome.errors.length === 1 ? 'One link couldn’t be added' : `${String(outcome.errors.length)} links couldn’t be added`}</p>
          <ul className="mt-2 space-y-1 text-xs">
            {outcome.errors.map(({ source, message }) => (
              <li key={source} className="flex gap-2">
                <span className="min-w-0 flex-1 truncate font-mono text-fg-muted">{source}</span>
                <span className="shrink-0 text-danger">{message}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

/** The choices left: rename, add anyway, or skip; or only Done. */
export function AddOutcomeFooter({
  outcome,
  submitting,
  onClose,
  onRetry,
}: {
  outcome: Outcome;
  submitting: boolean;
  onClose: () => void;
  onRetry: (over: Partial<AddRequest>) => void;
}) {
  const direct = outcome.conflicts.filter((c) => !c.torrent).map((c) => c.source);
  const again = outcome.duplicates.filter((d) => d.duplicate.canAddAnyway).map((d) => d.source);
  const torrents = outcome.conflicts.some((c) => c.torrent);
  if (direct.length > 0) {
    return (
      <>
        <Button variant="ghost" onClick={onClose}>
          Skip
        </Button>
        <Button variant="primary" busy={submitting} onClick={() => { onRetry({ ...only(direct, outcome.request), onConflict: 'rename' }); }}>
          Keep both (rename)
        </Button>
      </>
    );
  }
  if (again.length > 0) {
    return (
      <>
        <Button variant="ghost" onClick={onClose}>
          Skip
        </Button>
        <Button variant="primary" busy={submitting} onClick={() => { onRetry({ ...only(again, outcome.request), allowDuplicate: true }); }}>
          Add anyway
        </Button>
      </>
    );
  }
  return (
    <Button variant={torrents ? 'ghost' : 'primary'} onClick={onClose}>
      {torrents ? 'Skip' : 'Done'}
    </Button>
  );
}
