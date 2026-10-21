import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import { parseAppInfo, windowHeading, type AppInfo } from './appInfo';

export function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke('app_info')
      .then((value) => {
        setInfo(parseAppInfo(value));
      })
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : String(e));
      });
  }, []);

  return (
    <main className="shell">
      <h1>{info ? windowHeading(info) : 'Velocity Download Manager (dev)'}</h1>
      {error ? (
        <p role="alert">Could not reach the native shell: {error}</p>
      ) : (
        <p>Development shell. Features arrive through the issues on the project board.</p>
      )}
    </main>
  );
}
