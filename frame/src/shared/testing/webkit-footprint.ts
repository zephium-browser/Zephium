export type WebKitFootprint = {
  /** Physical footprint of every WebKit process this run started, in MB. */
  total: number;
  processes: { pid: number; name: string; mb: number; cpu: string }[];
};
