const PATHS: Record<string, string> = {
  spark: '<path d="M12 2l2.4 7.2L22 12l-7.6 2.8L12 22l-2.4-7.2L2 12l7.6-2.8z"/>',
  menu: '<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>',
  plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
  sliders: '<path d="M21 4h-7"/><path d="M10 4H3"/><path d="M21 12h-9"/><path d="M8 12H3"/><path d="M21 20h-5"/><path d="M12 20H3"/><path d="M14 2v4"/><path d="M8 10v4"/><path d="M16 18v4"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="M4.93 4.93l1.41 1.41"/><path d="M17.66 17.66l1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="M6.34 17.66l-1.41 1.41"/><path d="M19.07 4.93l-1.41 1.41"/>',
  moon: '<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>',
  paperclip: '<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
  send: '<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>',
  down: '<path d="M12 5v14"/><path d="M19 12l-7 7-7-7"/>',
  undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>',
  redo: '<path d="M15 14l5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>',
  diff: '<path d="M12 3v6"/><path d="M9 6h6"/><path d="M12 21v-6"/><path d="M9 18h6"/><path d="M5 9.5v5a1.5 1.5 0 0 1 -1.5 1.5"/><path d="M19 14.5v-5A1.5 1.5 0 0 1 20.5 8"/>',
  command: '<path d="M9 9V6a3 3 0 1 0 -3 3h3zm0 0v6m0-6h6m-6 6H6a3 3 0 1 0 3 3v-3zm6-6h3a3 3 0 1 0 -3-3v3zm0 0v6m0 0h3a3 3 0 1 1 -3 3v-3z"/>',
  trash: '<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>',
  folder: '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>',
  file: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/>',
  x: '<path d="M18 6L6 18"/><path d="M6 6l12 12"/>',
  up: '<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>',
  check: '<path d="M20 6L9 17l-5-5"/>',
  box: '<path d="M21 8l-9-5-9 5v8l9 5 9-5z"/><path d="M3 8l9 5 9-5"/><path d="M12 13v9"/>',
  play: '<path d="M7 4l13 8-13 8z"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  download: '<path d="M12 3v12"/><path d="M7 10l5 5 5-5"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/>',
  warn: '<path d="M12 3L2 21h20z"/><path d="M12 10v5"/><path d="M12 18h.01"/>',
  terminal: '<path d="M4 17l6-5-6-5"/><path d="M12 19h8"/>',
  server: '<rect x="2" y="3" width="20" height="7" rx="1.5"/><rect x="2" y="14" width="20" height="7" rx="1.5"/><path d="M6 6.5h.01"/><path d="M6 17.5h.01"/>',
  skill: '<path d="M12 2l2.6 5.3 5.9.9-4.2 4.1 1 5.8L12 15.4 6.7 18.1l1-5.8L3.5 8.2l5.9-.9z"/>',
  crew: '<circle cx="8" cy="9" r="3.2"/><circle cx="16.5" cy="10.5" r="2.6"/><path d="M2.5 19c.6-3.2 2.8-5 5.5-5s4.9 1.8 5.5 5"/><path d="M14.5 14.6c2.3.3 3.9 1.9 4.4 4.4"/>',
  chevron: '<path d="M6 9l6 6 6-6"/>',
};

export function iconSvg(name: string) {
  return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${PATHS[name] || ""}</svg>`;
}

export function Icon({ name, className }: { name: string; className?: string }) {
  return (
    <span
      className={className ? "inline-flex [&>svg]:size-4 [&>svg]:shrink-0 " + className : "inline-flex [&>svg]:size-4 [&>svg]:shrink-0"}
      dangerouslySetInnerHTML={{ __html: iconSvg(name) }}
    />
  );
}
