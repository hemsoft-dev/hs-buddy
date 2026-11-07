import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { act, render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import { AppContentRouter } from './AppContentRouter'
import { useAppTabs } from '../hooks/useAppTabs'

// Capture the exact CDP expressions used by the installed harness, without
// starting any installer, Linux helper, backend or application process.
const probe = `
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location('lifecycle',sys.argv[1])
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class Renderer:
    def evaluate(self, expression):
        self.expression=expression
renderer=Renderer();module.open_crew_project(renderer);navigate=renderer.expression
module.crew_visible(renderer)
print(json.dumps(dict(navigate=navigate,visible=renderer.expression,project=module.PROJECT,message=module.MESSAGE)))
`
const fixture: { navigate: string; visible: string; project: string; message: string } = JSON.parse(
  execFileSync(
    process.platform === 'win32' ? 'python' : 'python3',
    ['-c', probe, resolve('scripts/native-smoke/linux-installed-lifecycle.py')],
    { encoding: 'utf8' }
  )
)
const noop = () => {}

function InstalledRenderer() {
  const tabs = useAppTabs({ onViewOpen: noop })
  if (tabs.activeViewId === 'dashboard') return <div>Initial dashboard</div>
  return (
    <AppContentRouter
      activeViewId={tabs.activeViewId}
      prCounts={{}}
      onNavigate={tabs.openTab}
      onSectionChange={noop}
      onOpenTab={tabs.openTab}
      onCloseView={tabs.closeView}
      onPRCountChange={noop}
    />
  )
}

it('restores the installed fixture through the real tab listener, router and Crew view', async () => {
  const project = {
    id: fixture.project,
    displayName: fixture.project,
    localPath: '/synthetic/profile',
    gitRoot: '/synthetic/profile',
    githubSlug: 'fixture-org/lifecycle',
    defaultBranch: 'main',
    lastOpenedAt: 1,
    lastActiveAt: 1,
  }
  const getSession = vi.fn().mockResolvedValue({
    id: 'synthetic-session',
    projectId: project.id,
    status: 'error',
    conversationHistory: [{ role: 'user', content: fixture.message, timestamp: 1 }],
    changedFiles: [],
    createdAt: 1,
    updatedAt: 1,
  })
  Object.defineProperty(window, 'crew', {
    configurable: true,
    value: { listProjects: vi.fn().mockResolvedValue([project]), getSession },
  })
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, value: noop })
  render(<InstalledRenderer />)
  await act(async () => {
    window.eval(fixture.navigate)
  })
  expect(await screen.findByRole('heading', { level: 2, name: project.displayName })).toBeVisible()
  expect(await screen.findByText(fixture.message)).toBeVisible()
  expect(await screen.findByText('Session encountered an error')).toBeVisible()
  expect(getSession).toHaveBeenCalledWith(project.id)
  // Run the installed wait predicate on the real rendered view, then remove
  // each required part. Sidebar/tab text alone cannot satisfy restoration.
  expect(window.eval(fixture.visible)).toBe(true)
  for (const element of [
    screen.getByRole('heading', { level: 2, name: project.displayName }),
    screen.getByText(fixture.message),
    screen.getByText('Session encountered an error'),
  ]) {
    const text = element.textContent
    element.textContent = 'missing restored state'
    expect(window.eval(fixture.visible)).toBe(false)
    element.textContent = text
  }
})
