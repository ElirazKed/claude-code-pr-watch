import '@fontsource/ibm-plex-sans/latin-400.css'
import '@fontsource/ibm-plex-sans/latin-500.css'
import '@fontsource/ibm-plex-sans/latin-600.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-500.css'
import '@fontsource/space-grotesk/latin-500.css'
import '@fontsource/space-grotesk/latin-700.css'
import './themes.css'
import './styles.css'

import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { takeKey } from './api'
import { App } from './App'
import { applyTheme, readTheme } from './theme'

applyTheme(readTheme())

const key = takeKey()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {key ? (
      <App serverKey={key} />
    ) : (
      <div className="lone">
        <p>
          Open the link <code>prw web</code> printed: it holds the key to this run.
        </p>
      </div>
    )}
  </StrictMode>,
)
