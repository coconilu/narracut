export type IconName =
  | "alert"
  | "archive"
  | "check"
  | "chevron-left"
  | "clock"
  | "download"
  | "folder"
  | "history"
  | "more"
  | "play"
  | "refresh"
  | "settings"
  | "square"
  | "x";

interface IconProps {
  readonly name: IconName;
  readonly className?: string;
  readonly size?: number;
}

export function Icon({ name, className = "icon", size = 18 }: IconProps) {
  const content = iconPaths[name];
  return (
    <svg
      aria-hidden="true"
      className={className}
      height={size}
      viewBox="0 0 24 24"
      width={size}
    >
      {content}
    </svg>
  );
}

const iconPaths: Record<IconName, React.ReactNode> = {
  alert: (
    <>
      <path d="M12 3.4 21 19H3L12 3.4Z" />
      <path d="M12 9v4.6" />
      <path d="M12 17.1h.01" />
    </>
  ),
  archive: (
    <>
      <path d="M4 7.2h16v12H4z" />
      <path d="M3 4h18v3.2H3z" />
      <path d="M9 11h6" />
    </>
  ),
  check: <path d="m5 12.5 4.1 4.1L19 6.8" />,
  "chevron-left": <path d="m15 18-6-6 6-6" />,
  clock: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7.5V12l3.2 2" />
    </>
  ),
  download: (
    <>
      <path d="M12 3v11" />
      <path d="m7.5 10 4.5 4.5 4.5-4.5" />
      <path d="M5 20h14" />
    </>
  ),
  folder: <path d="M3 6.8h6l2 2H21v9.7H3z" />,
  history: (
    <>
      <path d="M4 7v5h5" />
      <path d="M5.2 16.7A8.5 8.5 0 1 0 4 8" />
      <path d="M12 7.5V12l3 1.8" />
    </>
  ),
  more: (
    <>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </>
  ),
  play: <path className="icon-fill" d="m8 5 11 7-11 7Z" />,
  refresh: (
    <>
      <path d="M20 7v5h-5" />
      <path d="M4 17v-5h5" />
      <path d="M6.1 8.2A7.5 7.5 0 0 1 19.5 11M4.5 13A7.5 7.5 0 0 0 18 15.8" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19 12a7.1 7.1 0 0 0-.1-1l2-1.5-2-3.4-2.4 1a8 8 0 0 0-1.7-1L14.5 3h-5l-.4 3.1a8 8 0 0 0-1.7 1L5 6.1 3 9.5 5 11a7.1 7.1 0 0 0 0 2l-2 1.5 2 3.4 2.4-1a8 8 0 0 0 1.7 1l.4 3.1h5l.4-3.1a8 8 0 0 0 1.7-1l2.4 1 2-3.4-2-1.5a7.1 7.1 0 0 0 .1-1Z" />
    </>
  ),
  square: <rect x="6" y="6" width="12" height="12" rx="1.5" />,
  x: (
    <>
      <path d="m6 6 12 12" />
      <path d="M18 6 6 18" />
    </>
  ),
};
