import { describe, expect, test } from 'claude-code/testing'
import { ago } from './pr'

// Throwaway: fails on purpose so the pane's "Fix with Claude" button has a real failed run to
// hand over. This PR is closed, never merged.
describe('demo for Fix with Claude', () => {
  test('ago() says "1 hour ago" for an hour', async () => {
    expect(ago(3_600_000)).toBe('1 hour ago')
  })
})
