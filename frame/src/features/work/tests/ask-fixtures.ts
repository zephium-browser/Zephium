import type {
  WorkExecutionFact,
  WorkHumanPageV1,
  WorkPageV1,
  WorkStepFact,
} from "$shared/ipc/bindings";
import { projection } from "./environment-fixtures";

const running = (id: string, kind: WorkStepFact["kind"], extra: Partial<WorkStepFact> = {}) =>
  ({ id, turn: 2, kind, status: "running", ...extra }) as WorkStepFact;

/** A page task on Slack held on its Send, as the loopback approve mode leaves it. */
export const slackSend = running("confirm-slack", {
  kind: "confirm",
  confirm: {
    site: "slack.com",
    category: "communication",
    headline: "Send to #design as you?",
    action: "press Send",
    text: "Morning! The new onboarding build is up on staging. The empty states and the sign-in sheet are done; the pricing page still uses the old cards. Could you look before Thursday's review?",
    page: "read-slack",
    provenance: ["notion.so"],
  },
});

/** Airbnb's Request to book, with the total and dates read from the page. */
export const airbnbBook = running("confirm-airbnb", {
  kind: "confirm",
  confirm: {
    site: "airbnb.co.uk",
    category: "purchase",
    headline: "Request to book for $4,212?",
    action: "press Request to book",
    facts: [
      { label: "Dates", value: "Jan 5 – Feb 2, 2027" },
      { label: "Guests", value: "1 adult" },
      { label: "Monthly stay discount", value: "−$1,030" },
      { label: "Total (USD)", value: "$4,212" },
    ],
    page: "01M3CV537PTJCSWZVY4Q3Y4A6B",
  },
});

/** A Notion page that saves as it is typed, with the run-wide allowance offered. */
export const notionEdit = running("confirm-notion", {
  kind: "confirm",
  confirm: {
    site: "notion.so",
    category: "edit",
    headline: "Save changes on notion.so?",
    action: "type into Page",
    text: "## Decisions\n- Ship the onboarding build on Thursday\n- Keep the old pricing cards until the review",
    run_option: true,
  },
});

export const typeSearch = running("confirm-type", {
  kind: "confirm",
  confirm: {
    site: "collector.example",
    category: "type",
    headline: "Type on collector.example?",
    action: "type into Search",
    text: "aisle seat WAW-SFO",
    run_option: true,
  },
});

export const deleteRepo = running("confirm-delete", {
  kind: "confirm",
  confirm: {
    site: "github.com",
    category: "destructive",
    headline: "Delete crynta/old-landing?",
    action: "press Delete this repository",
  },
});

export const slackTask = running("read-slack", {
  kind: "read",
  url: "https://app.slack.com/client/T0/C0",
  goal: "Read #design since Monday and draft a reply to Anna",
});

export const slackEntry = running("entry-slack", {
  kind: "ask",
  prompt: "Work in your Slack? Read #design since Monday and draft a reply to Anna.",
  options: ["Allow", "Always for Slack", "Not now"],
});

export const dayTasks = [
  slackTask,
  running("read-gmail", {
    kind: "read",
    url: "https://mail.google.com/mail/u/0/",
    goal: "Today's mail",
  }),
  running("read-calendar", {
    kind: "read",
    url: "https://calendar.google.com/calendar/r/day",
    goal: "Today's calendar",
  }),
];

export const dayEntry = running("entry-day", {
  kind: "ask",
  prompt: "Work in your Slack, Gmail and Calendar?",
  options: ["Allow", "Always for Slack, Gmail and Calendar", "Not now"],
});

export const historyAsk = running("ask-history", {
  kind: "ask",
  prompt: "Use your history? Looking for the flight comparison you read last week.",
  options: ["Allow", "Not now"],
});

export const notesAsk = running("ask-notes", {
  kind: "ask",
  prompt: "Use your notes? Your notes on the YC batch may already hold the dates.",
  options: ["Allow", "Not now"],
});

export const tabsAsk = running("ask-tabs", {
  kind: "ask",
  prompt: "Use your open tabs? You have flight searches open that could save a search.",
  options: ["Allow", "Not now"],
});

export const githubAsk = running("ask-github", {
  kind: "ask",
  prompt: "Use GitHub (gh)? The issue and the PR are faster to read through gh.",
  options: ["Use GitHub", "Use the website instead"],
});

export const folderAsk = running(
  "ask-folder",
  {
    kind: "ask",
    prompt: "Read Lunios?",
    options: ["Allow for this work", "Not now"],
    purpose: "folder",
  },
  { local: { folder: "/Users/crynta/Dev/Lunios" } as WorkStepFact["local"] },
);

/** An address the agent wrote itself after reading the person's history. */
export const addressAsk = running("ask-address", {
  kind: "ask",
  prompt: "https://warsaw-sfo-flights.collector.example/trips/aisle-seat",
  options: ["Open", "Allow collector.example for this request", "Don\u2019t open"],
  purpose: "address",
});

/** A folder the run needs as it works, chosen in the system's panel. */
export const documentsAsk = running(
  "ask-documents",
  {
    kind: "ask",
    prompt: "Allow Documents? To save binary-search.md there.",
    options: ["Choose folder…", "Not now"],
    purpose: "folder",
  },
  { local: { folder: "/Users/crynta/Documents" } as WorkStepFact["local"] },
);

export const budgetAsk = running("ask-budget", {
  kind: "ask",
  prompt: "What's your budget for the stay, for the whole month?",
  options: ["Under $3,000", "$3,000 – $5,000", "Over $5,000, if it's close to the office"],
});

export const settled = (step: WorkStepFact, patch: Partial<WorkStepFact>): WorkStepFact =>
  ({ ...structuredClone(step), ...patch }) as WorkStepFact;

export const decided = (
  step: WorkStepFact,
  decision: "approved" | "declined" | "allowed_for_run",
  status: WorkStepFact["status"],
  note?: string,
): WorkStepFact => {
  const copy = structuredClone(step);
  if (copy.kind.kind === "confirm") copy.kind.confirm.decision = decision;
  return { ...copy, status, ...(note ? { note } : {}) };
};

export const answered = (step: WorkStepFact, answer: string): WorkStepFact => {
  const copy = structuredClone(step);
  if (copy.kind.kind === "ask") copy.kind.answer = answer;
  return { ...copy, status: "succeeded" };
};

export const tripPage: WorkPageV1 = {
  execution: "execution",
  attempt: "01M3CV1H7HRABAGVH1T8HXH2DD",
  step: "01M3CV537PTJCSWZVY4Q3Y4A6B",
  url: "https://www.airbnb.co.uk/rooms/965072398758262802",
  live: true,
  frame: { generation: 1, width: 1280, height: 800 },
};

export const notionWall: WorkHumanPageV1 = {
  id: { attempt: "attempt", step: "read-notion", generation: 1 },
  phase: "waiting_for_human",
  reason: "sign_in",
  remaining_millis: 540_000,
  document_revision: "1",
  can_continue: true,
};

export const notionTask = running("read-notion", {
  kind: "read",
  url: "https://www.notion.so/login",
  goal: "Find the launch checklist",
});

/** A live agent run holding these steps, on the shared projection fixture. */
export function runWith(steps: WorkStepFact[]): WorkExecutionFact {
  const base = structuredClone(projection.executions[0]!);
  return {
    ...base,
    status: "running",
    authorization: "user_directed_agent",
    attempts: [{ id: "attempt", node: "node", status: "running", usage: null }],
    spec: {
      ...base.spec,
      nodes: [
        {
          ...base.spec.nodes[0]!,
          capability: {
            kind: "agent",
            grant: {
              provider: "open_ai",
              model: "gpt-5.6-luna",
              max_turns: 10,
              max_steps: 32,
              browse_hops: 4,
            },
          },
        },
      ],
    },
    steps,
  };
}
