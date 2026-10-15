import { useState, useEffect } from 'react'
import { RefreshCw, Key, ExternalLink, CheckCircle, AlertCircle } from 'lucide-react'
import { IPC_INVOKE } from '../../ipc/contracts'
import './SettingsShared.css'

function ApiKeyStatus({ isConfigured }: { isConfigured: boolean }) {
  return (
    <div className="settings-status-row">
      {isConfigured ? (
        <>
          <CheckCircle size={14} className="settings-status-icon-ok" />
          <span className="settings-status-text">
            Configured: pollen data will appear in your weather card.
          </span>
        </>
      ) : (
        <>
          <AlertCircle size={14} className="settings-status-icon-muted" />
          <span className="settings-status-text settings-status-muted">
            Not configured: add an API key to see pollen data.
          </span>
        </>
      )}
    </div>
  )
}

function SaveButton({ saving }: { saving: boolean }) {
  if (saving) {
    return (
      <>
        <RefreshCw className="spin" size={14} />
        Saving…
      </>
    )
  }
  return <>Save Key</>
}

function SettingsWeatherActions({
  saving,
  isDirty,
  apiKey,
  isConfigured,
  statusError,
  onSave,
  onClear,
}: {
  saving: boolean
  isDirty: boolean
  apiKey: string
  isConfigured: boolean
  statusError: string | null
  onSave: () => void
  onClear: () => void
}) {
  return (
    <>
      <div className="button-group">
        <button
          type="button"
          className="settings-btn settings-btn-primary"
          onClick={onSave}
          disabled={saving || !isDirty || !apiKey.trim()}
        >
          <SaveButton saving={saving} />
        </button>
        {isConfigured && (
          <button
            type="button"
            className="settings-btn settings-btn-secondary"
            onClick={onClear}
            disabled={saving}
          >
            Clear Key
          </button>
        )}
      </div>
      {statusError ? (
        <p role="alert" className="settings-status-text">
          {statusError}
        </p>
      ) : (
        <ApiKeyStatus isConfigured={isConfigured} />
      )}
    </>
  )
}

export function SettingsWeather() {
  const [apiKey, setApiKey] = useState('')
  const [isConfigured, setIsConfigured] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [showKey, setShowKey] = useState(false)

  useEffect(() => {
    let active = true
    window.ipcRenderer
      .invoke(IPC_INVOKE.CONFIG_GET_POLLEN_API_KEY)
      .then((status: { configured: boolean; error?: string } | null | undefined) => {
        if (!status || typeof status.configured !== 'boolean' || status.error) {
          throw new Error('Secure credential status unavailable')
        }
        if (active) setIsConfigured(status.configured)
      })
      .catch(() => {
        if (active)
          setError(
            'Could not read the saved key. Unlock secure storage, or save your preferred key again to finish migration.'
          )
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
    }
  }, [])

  const handleSave = async () => {
    setSaving(true)
    setError(null)
    try {
      const result = await window.ipcRenderer.invoke(
        IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY,
        apiKey.trim()
      )
      if (!result?.success) throw new Error('Secure save failed')
      setIsConfigured(true)
      setApiKey('')
      setShowKey(false)
    } catch (_: unknown) {
      setError(
        'Could not save the key. Check secure storage and configuration-file permissions, then try again.'
      )
    } finally {
      setSaving(false)
    }
  }

  const handleClear = async () => {
    setSaving(true)
    setError(null)
    try {
      const result = await window.ipcRenderer.invoke(IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY, '')
      if (!result?.success) throw new Error('Secure clear failed')
      setApiKey('')
      setIsConfigured(false)
      setShowKey(false)
    } catch (_: unknown) {
      setError(
        'Could not clear the key. Check secure storage and configuration-file permissions, then try again.'
      )
    } finally {
      setSaving(false)
    }
  }

  const isDirty = apiKey.trim().length > 0

  if (loading) {
    return (
      <div className="settings-page">
        <div className="settings-loading">
          <RefreshCw className="spin" size={24} />
          <p>Loading weather settings…</p>
        </div>
      </div>
    )
  }

  return (
    <div className="settings-page">
      <div className="settings-page-header">
        <h2>Weather</h2>
        <p className="settings-page-description">Configure pollen data and weather preferences.</p>
      </div>

      <div className="settings-page-content">
        <div className="settings-section">
          <div className="section-header">
            <h3>
              <Key size={16} />
              Pollen API Key
            </h3>
          </div>
          <p className="section-description">
            Use a Google Cloud API key with the Pollen API enabled to see tree, grass, and weed
            pollen levels in your weather card.
          </p>

          <div className="settings-field-group">
            <label htmlFor="pollen-api-key" className="settings-label">
              Google Cloud API Key
            </label>
            <div className="settings-input-row">
              <input
                aria-label="Enter your Google Cloud API key…"
                id="pollen-api-key"
                type={showKey ? 'text' : 'password'}
                className="settings-input"
                placeholder="Enter your Google Cloud API key…"
                value={apiKey}
                disabled={saving}
                onChange={e => {
                  setApiKey(e.target.value)
                  setError(null)
                }}
                autoComplete="off"
              />
              <button
                type="button"
                className="settings-btn settings-btn-secondary"
                onClick={() => setShowKey(!showKey)}
                title={showKey ? 'Hide key' : 'Show key'}
                disabled={saving}
              >
                {showKey ? 'Hide' : 'Show'}
              </button>
            </div>
          </div>

          <SettingsWeatherActions
            saving={saving}
            isDirty={isDirty}
            apiKey={apiKey}
            isConfigured={isConfigured}
            statusError={error}
            onSave={handleSave}
            onClear={handleClear}
          />

          <div className="info-box">
            <p>
              <strong>Configure your API key:</strong>
            </p>
            <p>
              <a
                href="https://console.cloud.google.com/apis/library/pollen.googleapis.com"
                className="settings-link"
                onClick={e => {
                  e.preventDefault()
                  window.ipcRenderer.invoke(
                    'shell:open-external',
                    'https://console.cloud.google.com/apis/library/pollen.googleapis.com'
                  )
                }}
              >
                <ExternalLink size={12} />
                Enable the Google Pollen API
              </a>{' '}
              : enable the API in your Google Cloud project, then create an API key under
              Credentials.
            </p>
            <p className="mt-2">
              <strong>Privacy:</strong> Saved keys use your OS credential store. This form never
              displays a saved key. Pollen requests send the key only to Google.
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}
