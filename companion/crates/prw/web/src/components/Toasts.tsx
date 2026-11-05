export interface ToastItem {
  id: number
  text: string
  /** A PR back from Later by itself: shown a while longer, and marked. */
  isWake?: boolean
  isBad?: boolean
  undo?: () => void
}

/** What a change did, with its undo, and what came back by itself. */
export function Toasts({
  toasts,
  onDismiss,
}: {
  toasts: ToastItem[]
  onDismiss: (id: number) => void
}) {
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={toast.isWake ? 'toast wake' : toast.isBad ? 'toast bad' : 'toast'}
        >
          <span>{toast.text}</span>
          {toast.undo && (
            <button
              type="button"
              onClick={() => {
                onDismiss(toast.id)
                toast.undo?.()
              }}
            >
              Undo
            </button>
          )}
        </div>
      ))}
    </div>
  )
}
