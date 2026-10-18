import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const colors = readFileSync('src/index.css', 'utf8')
const appStyles = readFileSync('src/App.css', 'utf8')
const statusStyles = readFileSync('src/components/StatusBar.css', 'utf8')
const sidebarStyles = readFileSync('src/components/SidebarPanel.css', 'utf8')

function cssBlock(pattern: RegExp): string {
  const block = colors.match(pattern)?.[1]
  if (!block) throw new Error(`Missing CSS theme block: ${pattern}`)
  return block
}

function cssColor(block: string, property: string): string {
  const value = block.match(new RegExp(`--${property}:\\s*(#[0-9a-fA-F]{6})`))?.[1]
  if (!value) throw new Error(`Missing hexadecimal --${property} value`)
  return value
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map(offset => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255)
  const linear = channels.map(channel =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  )
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

function contrast(foreground: string, background: string): number {
  const values = [luminance(foreground), luminance(background)].sort((a, b) => b - a)
  return (values[0] + 0.05) / (values[1] + 0.05)
}

function blendWhite(hex: string, alpha: number): string {
  const channels = [1, 3, 5].map(offset => Number.parseInt(hex.slice(offset, offset + 2), 16))
  return `#${channels
    .map(channel =>
      Math.round(channel * (1 - alpha) + 255 * alpha)
        .toString(16)
        .padStart(2, '0')
    )
    .join('')}`
}

function blendColor(foreground: string, background: string, opacity: number): string {
  return `#${[1, 3, 5]
    .map(offset => {
      const front = Number.parseInt(foreground.slice(offset, offset + 2), 16)
      const back = Number.parseInt(background.slice(offset, offset + 2), 16)
      return Math.round(front * opacity + back * (1 - opacity))
        .toString(16)
        .padStart(2, '0')
    })
    .join('')}`
}

const themes = [
  cssBlock(/:root\s*\{([\s\S]*?)\n\}/),
  cssBlock(/\[data-theme='light'\]\s*\{([\s\S]*?)\n\}/),
]

describe('renderer shell contrast', () => {
  it.each(themes)('keeps informational sidebar empty states at WCAG AA contrast', theme => {
    const rule = sidebarStyles.match(/\.sidebar-item-empty\s*\{([^}]*)\}/s)?.[1]
    if (!rule) throw new Error('Missing sidebar empty-state style')
    const property = rule.match(/color:\s*var\(--([a-z-]+)\)/)?.[1]
    if (!property) throw new Error('Missing empty-state theme color')
    const opacity = Number(rule.match(/opacity:\s*([\d.]+)/)?.[1] ?? 1)
    const background = cssColor(theme, 'sidebar-bg')
    const foreground = blendColor(cssColor(theme, property), background, opacity)
    expect(contrast(foreground, background)).toBeGreaterThanOrEqual(4.5)
  })

  it.each(themes)('keeps loading and idle-status text at WCAG AA contrast', theme => {
    expect(
      contrast(cssColor(theme, 'text-secondary'), cssColor(theme, 'bg-primary'))
    ).toBeGreaterThanOrEqual(4.5)
    const statusForeground = cssColor(theme, 'statusbar-fg')
    const statusBackground = cssColor(theme, 'statusbar-bg')
    expect(contrast(statusForeground, statusBackground)).toBeGreaterThanOrEqual(4.5)
    expect(contrast(statusForeground, blendWhite(statusBackground, 0.08))).toBeGreaterThanOrEqual(
      4.5
    )
  })

  it('uses the qualified theme colors without reducing their opacity', () => {
    expect(appStyles).toMatch(/\.app-loading-message\s*\{[^}]*color: var\(--text-secondary\)/s)
    expect(appStyles).not.toMatch(/\.app-loading-detail\s*\{[^}]*opacity:/s)
    expect(statusStyles).toMatch(/\.status-item-sync-idle\s*\{[^}]*color: var\(--statusbar-fg/s)
    expect(statusStyles).toMatch(
      /\.status-item-sync-idle:hover\s*\{[^}]*color: var\(--statusbar-fg/s
    )
  })
})
