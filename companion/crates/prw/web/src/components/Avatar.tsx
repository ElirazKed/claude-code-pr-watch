/** A login's first two letters in a circle; ringed when it is yours. */
export function Avatar({ login, size = 20, isYours = false }: Props) {
  return (
    <span
      className={isYours ? 'avatar me' : 'avatar'}
      title={login || undefined}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.4) }}
    >
      {(login || '?').slice(0, 2).toUpperCase()}
    </span>
  )
}

interface Props {
  login: string
  size?: number
  isYours?: boolean
}
