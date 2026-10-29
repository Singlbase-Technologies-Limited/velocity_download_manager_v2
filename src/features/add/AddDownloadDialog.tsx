import { useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, FileUp, FolderOpen, Globe, KeyRound, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { toast } from 'sonner';
import { cleartextWarning } from '../../capabilities';
import { Button } from '../../components/Button';
import { Checkbox } from '../../components/Checkbox';
import { Dialog } from '../../components/Dialog';
import { Field } from '../../components/Field';
import { Input } from '../../components/Input';
import { Select } from '../../components/Select';
import { Textarea } from '../../components/Textarea';
import type { AddRequest, AddResult } from '../../contract';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { formatBytes } from '../../lib/format';
import { useCapabilities, useCategories } from '../../lib/queries';
import { nameForDownload } from '../../naming';
import { useUiStore, type AddPrefill } from '../../stores/ui';
import { showDownload } from '../downloads/showDownload';
import { AddOutcomeFooter, AddOutcomeView } from './AddOutcomeView';
import { plural, sortOutcome, type Outcome } from './outcome';
import { linkKind, linkProblem, MAX_ADD, parseChecksum, parseHeaders, splitLinks } from './addLogic';

const AUTOMATIC = '';

const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => { setDebounced(value); }, ms);
    return () => { clearTimeout(t); };
  }, [value, ms]);
  return debounced;
}

/** The Add download dialog (#60, V1 #42): one form for links, magnet links and `.torrent` files. */
export function AddDownloadDialog() {
  const { open, prefill } = useUiStore((s) => s.addDialog);
  const closeAdd = useUiStore((s) => s.closeAdd);
  // A new prefill, from a drop or the browser, starts a fresh form.
  return open ? <AddForm key={JSON.stringify(prefill)} prefill={prefill} onClose={closeAdd} /> : null;
}

function AddForm({ prefill, onClose }: { prefill: AddPrefill | null; onClose: () => void }) {
  const { data: categories } = useCategories();
  const { data: capabilities } = useCapabilities();

  const [text, setText] = useState(prefill?.links ?? '');
  const [torrentFiles, setTorrentFiles] = useState<string[]>(prefill?.torrentFiles ?? []);
  const [fileName, setFileName] = useState(prefill?.fileName ?? '');
  const [directory, setDirectory] = useState('');
  const [categoryId, setCategoryId] = useState(AUTOMATIC);
  const [createDirectory, setCreateDirectory] = useState(false);
  const [requireEncryption, setRequireEncryption] = useState(false);
  const [chooseFiles, setChooseFiles] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [connections, setConnections] = useState('');
  const [speedLimit, setSpeedLimit] = useState('');
  const [referrer, setReferrer] = useState(prefill?.referrer ?? '');
  const [headersText, setHeadersText] = useState('');
  const [userName, setUserName] = useState('');
  const [password, setPassword] = useState('');
  const [keyPath, setKeyPath] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [checksumText, setChecksumText] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const links = useMemo(() => (torrentFiles.length > 0 ? [] : splitLinks(text)), [text, torrentFiles]);
  const kinds = links.map(linkKind);
  const invalid = links.filter((l) => linkProblem(l, capabilities) !== null);
  const firstInvalid = invalid[0];
  const single = links.length === 1 && torrentFiles.length === 0 ? links[0] : undefined;
  const singleDirect = single !== undefined && (kinds[0] === 'http' || kinds[0] === 'ftp' || kinds[0] === 'sftp') ? single : undefined;
  const torrentMode = torrentFiles.length > 0 || kinds.some((k) => k === 'magnet' || k === 'torrentLink');
  const hasSftp = kinds.includes('sftp');
  const plainFtp = links.find((l) => cleartextWarning(l, false) !== null);
  const knownExtensions = useMemo(() => new Set((categories ?? []).flatMap((c) => c.extensions)), [categories]);
  const linkName = singleDirect === undefined ? null : nameForDownload(singleDirect, null, knownExtensions);
  const typedName = singleDirect === undefined || fileName.trim() === '' ? null : nameForDownload(singleDirect, fileName, knownExtensions);
  const headers = parseHeaders(headersText);
  const checksum = parseChecksum(checksumText);
  const tooMany = links.length + torrentFiles.length > MAX_ADD;

  // What adding would do: the folder, the category, and whether the name is taken (#31).
  // Debounced as one string, so an unchanged question is the same value between renders.
  const asked = useDebounced(JSON.stringify([singleDirect ?? '', directory.trim(), fileName.trim(), categoryId]), 300);
  const check = useQuery({
    queryKey: ['destination', asked] as const,
    queryFn: () => {
      const [url, dir, name, category] = JSON.parse(asked) as [string, string, string, string];
      return api.checkDestination(url, dir || null, url === '' ? null : name || null, category || null);
    },
    retry: false,
  });
  const dest = check.data;
  // A missing folder the user chose needs their consent; an automatic one is the settings' or the category's.
  const missingChosen = directory.trim() !== '' && dest !== undefined && !dest.directoryExists && !dest.automaticDirectory;
  const autoCategory = categoryId === AUTOMATIC && dest?.categoryId ? categories?.find((c) => c.id === dest.categoryId) : undefined;

  const nothing = links.length === 0 && torrentFiles.length === 0;
  const blocked = nothing || invalid.length > 0 || tooMany || headers.error !== null || checksum.error !== null || (missingChosen && !createDirectory);

  const browse = async () => {
    try {
      const picked = await api.chooseDirectory(directory.trim() || dest?.directory || null);
      if (picked) setDirectory(picked);
    } catch (e) {
      toast.error(toApiError(e).message);
    }
  };
  const pickTorrents = async () => {
    try {
      const picked = await api.chooseTorrents();
      if (picked.length > 0) setTorrentFiles(picked);
    } catch (e) {
      toast.error(toApiError(e).message);
    }
  };
  const pickKey = async () => {
    try {
      const picked = await api.chooseSshKey();
      if (picked) setKeyPath(picked);
    } catch (e) {
      toast.error(toApiError(e).message);
    }
  };

  const request = (over: Partial<AddRequest>): AddRequest => ({
    links,
    torrentFiles,
    directory: directory.trim() || null,
    fileName: singleDirect !== undefined && fileName.trim() !== '' ? fileName.trim() : null,
    categoryId: categoryId || null,
    createDirectory: missingChosen && createDirectory,
    onConflict: 'ask',
    allowDuplicate: false,
    startPaused: false,
    connections: Number(connections) > 0 ? Math.floor(Number(connections)) : null,
    speedLimitKib: Number(speedLimit) > 0 ? Math.floor(Number(speedLimit)) : 0,
    headers: headers.headers,
    referrer: referrer.trim() || null,
    credentials: userName.trim() ? { userName: userName.trim(), password } : null,
    sshKey: hasSftp && keyPath.trim() ? { path: keyPath.trim(), passphrase: passphrase || null } : null,
    checksum: checksum.checksum,
    requireEncryption: plainFtp !== undefined && requireEncryption,
    chooseFiles: torrentMode && chooseFiles,
    ...over,
  });

  /** Sends `req`; closes when everything was added, otherwise shows what happened to each. */
  const run = async (req: AddRequest, addedBefore: number) => {
    setFormError(null);
    setSubmitting(true);
    try {
      const results: AddResult[] = await api.addDownloads(req);
      const next = sortOutcome(results, req, addedBefore);
      if (next.conflicts.length === 0 && next.duplicates.length === 0 && next.errors.length === 0) {
        toast.success(next.added === 1 ? 'Download added' : `${plural(next.added, 'download')} added`);
        onClose();
        return;
      }
      setOutcome(next);
    } catch (e) {
      setFormError(toApiError(e).message);
    } finally {
      setSubmitting(false);
    }
  };

  const submit = (startPaused: boolean) => {
    if (blocked) return;
    void run(request({ startPaused }), 0);
  };

  const footer = outcome ? null : (
    <>
      <Button variant="ghost" onClick={onClose}>
        Cancel
      </Button>
      <Button disabled={blocked || submitting} onClick={() => { submit(true); }}>
        Add paused
      </Button>
      <Button variant="primary" busy={submitting} disabled={blocked} onClick={() => { submit(false); }}>
        Start download
      </Button>
    </>
  );

  const fromBrowser = prefill?.fromBrowser;
  return (
    <Dialog
      open
      onOpenChange={(o) => { if (!o) onClose(); }}
      title="Add download"
      description={fromBrowser ? 'Sent from your browser. Check the details, then start it.' : 'Paste links, one per line, or choose .torrent files.'}
      className="max-w-2xl"
      footer={
        outcome ? (
          <AddOutcomeFooter
            outcome={outcome}
            submitting={submitting}
            onClose={() => {
              if (outcome.added > 0) toast.success(outcome.added === 1 ? 'Download added' : `${plural(outcome.added, 'download')} added`);
              onClose();
            }}
            onRetry={(over) => void run(request(over), outcome.added)}
            onShow={(id) => {
              showDownload(id);
              onClose();
            }}
          />
        ) : (
          footer
        )
      }
    >
      {outcome ? (
        <AddOutcomeView outcome={outcome} />
      ) : (
        <div className="space-y-4">
          {fromBrowser ? (
            <div className="flex items-start gap-2.5 rounded-md bg-accent-soft px-3 py-2 text-xs">
              <Globe className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
              <div className="min-w-0">
                <p className="truncate">{fromBrowser.referrer ? `From ${fromBrowser.referrer}` : 'From your browser'}</p>
                <p className="text-fg-muted">
                  {[fromBrowser.fileSize !== null ? formatBytes(fromBrowser.fileSize) : null, fromBrowser.mime].filter(Boolean).join(' · ') || 'The browser didn’t say how large it is.'}
                </p>
              </div>
            </div>
          ) : null}

          {torrentFiles.length > 0 ? (
            <div className="space-y-2 rounded-lg border border-border p-3">
              <div className="flex items-center justify-between gap-2">
                <p className="text-xs text-fg-muted">{torrentFiles.length === 1 ? 'One .torrent file. It is added with all its files.' : `${String(torrentFiles.length)} .torrent files. Each is added with all its files.`}</p>
                <Button size="sm" variant="ghost" onClick={() => { setTorrentFiles([]); }}>
                  Use links instead
                </Button>
              </div>
              <ul aria-label="Torrent files" className="max-h-40 space-y-1 overflow-y-auto text-xs">
                {torrentFiles.map((f) => (
                  <li key={f} className="truncate" title={f}>
                    {baseName(f)}
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <>
              <Field
                label="Links"
                error={
                  firstInvalid !== undefined
                    ? `${invalid.length === 1 ? '' : `${String(invalid.length)} links can’t be added. `}${firstInvalid}: ${linkProblem(firstInvalid, capabilities) ?? ''}`
                    : tooMany
                      ? `At most ${String(MAX_ADD)} links can be added at once.`
                      : null
                }
                hint={
                  kinds.includes('torrentLink')
                    ? 'Velocity fetches the .torrent file first, then adds the torrent.'
                    : links.length > 1
                      ? `${String(links.length)} links`
                      : 'HTTP, HTTPS, FTP, FTPS and SFTP links, magnet links and links to .torrent files.'
                }
              >
                {(p) => (
                  <Textarea {...p} autoFocus value={text} onChange={(e) => { setText(e.target.value); }} placeholder="https://example.com/file.zip" rows={links.length > 1 ? 5 : 3} />
                )}
              </Field>
              <div className="flex items-center gap-2 text-xs text-fg-muted">
                <Button size="sm" onClick={() => void pickTorrents()}>
                  <FileUp aria-hidden /> Choose .torrent files…
                </Button>
                <span>or drop them on the window.</span>
              </div>
            </>
          )}

          <div className="grid grid-cols-2 gap-3">
            <Field
              label="File name"
              hint={
                torrentMode
                  ? 'Torrents keep the names in their metadata.'
                  : links.length > 1
                    ? 'Each link names its own file.'
                    : typedName?.ok && typedName.name !== null && typedName.name !== fileName.trim()
                      ? `Saved as “${typedName.name}”`
                      : undefined
              }
              error={typedName && !typedName.ok ? 'This name can’t be used for a file.' : null}
            >
              {(p) => (
                <Input
                  {...p}
                  value={fileName}
                  disabled={singleDirect === undefined}
                  onChange={(e) => { setFileName(e.target.value); }}
                  placeholder={singleDirect === undefined ? '' : linkName?.ok && linkName.name !== null ? linkName.name : 'Named by the server'}
                />
              )}
            </Field>
            <Field label="Category">
              {(p) => (
                <Select {...p} value={categoryId} onChange={(e) => { setCategoryId(e.target.value); }}>
                  <option value={AUTOMATIC}>{autoCategory ? `Automatic: ${autoCategory.name}` : 'Automatic (by file type)'}</option>
                  {(categories ?? []).map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </Select>
              )}
            </Field>
          </div>

          <Field
            label="Save to"
            error={missingChosen ? 'This folder doesn’t exist. Choose another, or create it below.' : check.isError ? toApiError(check.error).message : null}
            hint={dest && directory.trim() === '' ? <span>{`Saves to ${dest.directory}`}</span> : undefined}
          >
            {(p) => (
              <div className="flex gap-2">
                <Input {...p} value={directory} onChange={(e) => { setDirectory(e.target.value); }} placeholder="The automatic folder" />
                <Button onClick={() => void browse()}>
                  <FolderOpen aria-hidden /> Browse
                </Button>
              </div>
            )}
          </Field>
          {missingChosen ? (
            <Checkbox checked={createDirectory} onChange={setCreateDirectory}>
              Create this folder
            </Checkbox>
          ) : null}
          {singleDirect !== undefined && dest?.fileName && (dest.fileExists || dest.partialDownloadExists) ? (
            <p className="flex items-center gap-2 rounded-md bg-warning-soft px-3 py-2 text-xs text-warning">
              <TriangleAlert className="size-4 shrink-0" aria-hidden />
              {`“${dest.fileName}” is already in this folder. You’ll be asked what to do when you start.`}
            </p>
          ) : null}

          {plainFtp !== undefined ? (
            <div className="space-y-1.5">
              <Checkbox checked={requireEncryption} onChange={setRequireEncryption}>
                Require encryption (FTPS)
              </Checkbox>
              {cleartextWarning(plainFtp, requireEncryption) ? (
                <p className="flex items-center gap-2 rounded-md bg-warning-soft px-3 py-2 text-xs text-warning">
                  <TriangleAlert className="size-4 shrink-0" aria-hidden />
                  {cleartextWarning(plainFtp, requireEncryption)}
                </p>
              ) : null}
            </div>
          ) : null}

          {torrentMode ? (
            <Checkbox checked={chooseFiles} onChange={setChooseFiles}>
              Choose files before downloading
            </Checkbox>
          ) : null}

          <button
            type="button"
            className="flex items-center gap-1 text-xs font-medium text-fg-muted hover:text-fg"
            aria-expanded={advanced}
            onClick={() => { setAdvanced((a) => !a); }}
          >
            {advanced ? <ChevronDown className="size-3.5" aria-hidden /> : <ChevronRight className="size-3.5" aria-hidden />} Advanced options
          </button>
          {advanced ? (
            <div className="space-y-3 rounded-lg border border-border p-3">
              <div className="grid grid-cols-2 gap-3">
                <Field label="Connections" hint="Empty uses the performance profile.">
                  {(p) => <Input {...p} type="number" min={1} max={64} value={connections} onChange={(e) => { setConnections(e.target.value); }} />}
                </Field>
                <Field label="Speed limit (KiB/s)" hint="Empty or 0 means no limit.">
                  {(p) => <Input {...p} type="number" min={0} value={speedLimit} onChange={(e) => { setSpeedLimit(e.target.value); }} />}
                </Field>
              </div>
              <Field label="Referrer">{(p) => <Input {...p} value={referrer} onChange={(e) => { setReferrer(e.target.value); }} placeholder="https://example.com/page" />}</Field>
              <Field label="Extra headers" error={headers.error} hint="One per line, like “Cookie: session=…”. Kept in memory only, never saved.">
                {(p) => <Textarea {...p} value={headersText} onChange={(e) => { setHeadersText(e.target.value); }} rows={2} placeholder="Name: value" />}
              </Field>
              <div className="grid grid-cols-2 gap-3">
                <Field label="User name">{(p) => <Input {...p} value={userName} onChange={(e) => { setUserName(e.target.value); }} autoComplete="off" />}</Field>
                <Field label="Password" hint="Kept in memory only, never saved.">
                  {(p) => <Input {...p} type="password" value={password} onChange={(e) => { setPassword(e.target.value); }} autoComplete="off" />}
                </Field>
              </div>
              {hasSftp ? (
                <div className="grid grid-cols-2 gap-3">
                  <Field label="SSH private key" hint="For SFTP links, with the user name above.">
                    {(p) => (
                      <div className="flex gap-2">
                        <Input {...p} value={keyPath} onChange={(e) => { setKeyPath(e.target.value); }} placeholder="None: log in with the password" />
                        <Button aria-label="Choose a key" title="Choose a key" onClick={() => void pickKey()}>
                          <KeyRound aria-hidden />
                        </Button>
                      </div>
                    )}
                  </Field>
                  <Field label="Key passphrase" hint="Only for a key that has one.">
                    {(p) => <Input {...p} type="password" value={passphrase} onChange={(e) => { setPassphrase(e.target.value); }} autoComplete="off" />}
                  </Field>
                </div>
              ) : null}
              <Field label="Checksum" error={checksum.error} hint="Checked when the download finishes, like “sha-256=…”.">
                {(p) => <Input {...p} value={checksumText} onChange={(e) => { setChecksumText(e.target.value); }} placeholder="sha-256=hexadecimal digest" className="font-mono text-xs" />}
              </Field>
            </div>
          ) : null}

          {formError ? (
            <p role="alert" className="text-xs text-danger">
              {formError}
            </p>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}
