import { useState, useEffect } from 'react'
import { useConfig } from '../../hooks/useConfig'
import {
  FileCode,
  RotateCcw,
  RefreshCw,
  CheckCircle,
  AlertTriangle,
  FolderOpen,
  HardDrive,
  Trash2,
} from 'lucide-react'
import { dataCache } from '../../services/dataCache'
import type { DataCacheStorageStats } from '../../services/dataCachePolicy'
import './SettingsShared.css'

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`
}

function ResetButtonLabel({
  isResetting,
  resetConfirm,
}: {
  isResetting: boolean
  resetConfirm: boolean
}) {
  if (isResetting)
    return (
      <>
        <RefreshCw className="spin" size={14} />
        Resetting…
      </>
    )
  if (resetConfirm)
    return (
      <>
        <AlertTriangle size={14} />
        Click Again to Confirm
      </>
    )
  return (
    <>
      <RotateCcw size={14} />
      Reset to Defaults
    </>
  )
}

export function SettingsAdvanced() {
  const { api, refresh, loading } = useConfig()
  const [storePath, setStorePath] = useState<string>('')
  const [openSuccess, setOpenSuccess] = useState(false)
  const [resetConfirm, setResetConfirm] = useState(false)
  const [isResetting, setIsResetting] = useState(false)
  const [resetError, setResetError] = useState<string | null>(null)
  const [cacheStats, setCacheStats] = useState<DataCacheStorageStats | null>(null)
  const [isClearingCache, setIsClearingCache] = useState(false)
  const [cacheClearError, setCacheClearError] = useState<string | null>(null)

  useEffect(() => {
    api.getStorePath().then(setStorePath)
    dataCache.getStorageStats().then(setCacheStats)
  }, [api])

  const handleOpenConfig = async () => {
    await api.openInEditor()
    setOpenSuccess(true)
    setTimeout(() => setOpenSuccess(false), 2000)
  }
  const handleReset = async () => {
    if (!resetConfirm) {
      setResetConfirm(true)
      setTimeout(() => setResetConfirm(false), 3000)
      return
    }
    setIsResetting(true)
    setResetError(null)
    let resetApplied = false
    try {
      const result = await api.reset()
      if (result?.success !== true) throw new Error('Reset was not acknowledged')
      resetApplied = true
      await refresh()
    } catch (_: unknown) {
      setResetError(
        resetApplied
          ? 'Settings were reset, but could not be reloaded. Reopen Settings or retry.'
          : 'Could not reset settings. Unlock secure storage, check file permissions, then retry.'
      )
    } finally {
      setIsResetting(false)
      setResetConfirm(false)
    }
  }
  const handleClearCache = async () => {
    setIsClearingCache(true)
    setCacheClearError(null)
    try {
      if (await dataCache.clear()) setCacheStats(await dataCache.getStorageStats())
      else setCacheClearError('Failed to clear cached data. Please try again.')
    } finally {
      setIsClearingCache(false)
    }
  }

  if (loading) {
    return (
      <div className="settings-page">
        <div className="settings-loading">
          <RefreshCw className="spin" size={24} />
          <p>Loading advanced settings…</p>
        </div>
      </div>
    )
  }

  return (
    <div className="settings-page">
      <div className="settings-page-header">
        <h2>Advanced</h2>
        <p className="settings-page-description">Access raw configuration and advanced options.</p>
      </div>
      <div className="settings-page-content">
        <div className="settings-section">
          <div className="section-header">
            <h3>
              <FileCode size={16} />
              Configuration File
            </h3>
          </div>
          <p className="section-description">
            Buddy stores its configuration in a JSON file using electron-store.
          </p>
          <div className="config-path-box">
            <FolderOpen size={16} />
            <code className="config-path">{storePath || 'Loading…'}</code>
          </div>
          <div className="button-group">
            <button
              type="button"
              className="settings-btn settings-btn-primary"
              onClick={handleOpenConfig}
            >
              {openSuccess ? (
                <>
                  <CheckCircle size={14} />
                  Opened!
                </>
              ) : (
                <>
                  <FileCode size={14} />
                  Open in Editor
                </>
              )}
            </button>
          </div>
          <p className="hint">
            The configuration file uses JSON Schema validation to ensure correctness.
          </p>
        </div>
        <div className="settings-section">
          <div className="section-header">
            <h3>
              <HardDrive size={16} />
              Cached Data
            </h3>
          </div>
          <p className="section-description">
            Buddy keeps a bounded local cache for faster startup and navigation.
          </p>
          <div className="config-path-box" aria-live="polite">
            <HardDrive size={16} />
            <span>
              {cacheStats
                ? `${cacheStats.entryCount} entries · ${formatBytes(cacheStats.totalBytes)}`
                : 'Loading cache size…'}
            </span>
          </div>
          <div className="button-group">
            <button
              type="button"
              className="settings-btn settings-btn-secondary"
              onClick={handleClearCache}
              disabled={isClearingCache}
            >
              {isClearingCache ? <RefreshCw className="spin" size={14} /> : <Trash2 size={14} />}
              {isClearingCache ? 'Clearing…' : 'Clear Cached Data'}
            </button>
          </div>
          {cacheClearError ? (
            <p className="hint" role="alert">
              {cacheClearError}
            </p>
          ) : null}
        </div>
        <div className="settings-section">
          <div className="section-header">
            <h3>
              <RotateCcw size={16} />
              Reset Configuration
            </h3>
          </div>
          <p className="section-description">Reset all settings to their default values.</p>
          <div className="danger-zone">
            <div className="danger-warning">
              <AlertTriangle size={16} />
              <span>This will remove all configured accounts and reset all preferences.</span>
            </div>
            <button
              type="button"
              className={`settings-btn ${resetConfirm ? 'settings-btn-danger' : 'settings-btn-secondary'}`}
              onClick={handleReset}
              disabled={isResetting}
            >
              <ResetButtonLabel isResetting={isResetting} resetConfirm={resetConfirm} />
            </button>
          </div>
          {resetError ? (
            <p className="hint" role="alert">
              {resetError}
            </p>
          ) : null}
        </div>
        <div className="settings-section">
          <h3>About Storage</h3>
          <div className="info-box">
            <p>
              <strong>Security Note:</strong>
            </p>
            <p>
              Authentication is handled by GitHub CLI (gh). Your tokens are stored securely in the
              system keychain, not in this configuration file.
            </p>
            <p className="mt-2">
              <strong>Location varies by OS:</strong>
            </p>
            <ul>
              <li>
                <strong>Windows:</strong> <code>%APPDATA%\hs-buddy\config.json</code>
              </li>
              <li>
                <strong>macOS:</strong>{' '}
                <code>~/Library/Application Support/hs-buddy/config.json</code>
              </li>
              <li>
                <strong>Linux:</strong> <code>~/.config/hs-buddy/config.json</code>
              </li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  )
}
