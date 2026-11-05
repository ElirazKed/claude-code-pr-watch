import { MODES, PALETTES, type Theme } from '../theme'

/** The palette, and light or dark. */
export function ThemePicker({
  theme,
  onPick,
  className = 'appearance',
}: {
  theme: Theme
  onPick: (theme: Theme) => void
  className?: string
}) {
  return (
    <div className={className}>
      <div className="lbl-sm">Theme</div>
      <div className="palettes" role="group" aria-label="Palette">
        {PALETTES.map((palette) => (
          <button
            key={palette.id}
            type="button"
            aria-pressed={theme.palette === palette.id}
            onClick={() => onPick({ ...theme, palette: palette.id })}
          >
            <span className="swatch" aria-hidden="true">
              {palette.swatch.map((colour) => (
                <i key={colour} style={{ background: colour }} />
              ))}
            </span>
            {palette.label}
          </button>
        ))}
      </div>
      <div className="modes" role="group" aria-label="Light or dark">
        {MODES.map((mode) => (
          <button
            key={mode.id}
            type="button"
            aria-pressed={theme.mode === mode.id}
            onClick={() => onPick({ ...theme, mode: mode.id })}
          >
            {mode.label}
          </button>
        ))}
      </div>
    </div>
  )
}
