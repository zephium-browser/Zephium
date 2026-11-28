; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; decision?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } | ({ kind: "read_file"; path: string; offset: number | null; limit: number | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; decision?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; new?: never; old?: never; options?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } | ({ kind: "search_files"; path: string; query: string; glob: string | null; regex: boolean | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; decision?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; prompt?: never; purpose?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } |
/**  A proposed whole-file write; `decision` is the person's answer. */
({ kind: "write_file"; path: string; content: string; decision?: boolean | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; cwd?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } |
/**  A proposed replacement of one exact passage. */
({ kind: "edit_file"; path: string; old: string; new: string; replacements?: WorkFileReplacementV1[]; decision?: boolean | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; offset?: never; options?: never; prompt?: never; purpose?: never; query?: never; regex?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } | ({ kind: "move_file"; from: string; to: string; decision: boolean | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; depth?: never; expected_revision?: never; followups?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; path?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; url?: never; verbatim?: never } | ({ kind: "delete_file"; path: string; decision: boolean | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } | ({ kind: "run_command"; cwd: string; command: string; timeout_secs: number | null; decision: boolean | null }) & { answer?: never; call?: never; collection?: never; confirm?: never; content?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; path?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; title?: never; to?: never; url?: never; verbatim?: never } | ({ kind: "finish";
/**  Up to three short next requests the person may choose. */
followups?: string[];
/**  The work's name, given by its first run: a short noun phrase. */
title?: string | null }) & { answer?: never; call?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; decision?: never; depth?: never; expected_revision?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; path?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; to?: never; url?: never; verbatim?: never } |
/**
 *  A step on a site that commits something for the person, held until
 *  they decide. Every field is Rust's reading of the page, never the
 *  model's words; the status is the receipt once it settles.
 */
({ kind: "confirm"; confirm: WorkConfirmV1_Serialize }) & { answer?: never; call?: never; collection?: never; command?: never; content?: never; cwd?: never; decision?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; path?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never } |
/**
 *  A call to an installed tool or connected service, settled as it is
 *  recorded; the note is its row for people.
 */
({ kind: "call"; call: WorkConnectionCallV1_Serialize }) & { answer?: never; collection?: never; command?: never; confirm?: never; content?: never; cwd?: never; decision?: never; depth?: never; expected_revision?: never; followups?: never; from?: never; glob?: never; goal?: never; limit?: never; new?: never; offset?: never; old?: never; options?: never; path?: never; prompt?: never; purpose?: never; query?: never; regex?: never; replacements?: never; resource?: never; text?: never; timeout_secs?: never; title?: never; to?: never; url?: never; verbatim?: never };

/**
 *  Closed counts for one browser step: durations, call counts and exact
 *  accounting beside the conservative ceiling in `usage`. Never page, model
 *  or provider text.
 */
export type WorkStepMeasurementsV1 = WorkStepMeasurementsV1_Serialize | WorkStepMeasurementsV1_Deserialize;

/**
 *  Closed counts for one browser step: durations, call counts and exact
 *  accounting beside the conservative ceiling in `usage`. Never page, model
 *  or provider text.
 */
export type WorkStepMeasurementsV1_Deserialize = {
	/**
	 *  Work revision assigned by Store when this page step settled. Never an
	 *  adapter clock or claim; absent on older or unfinished records.
	 */
	settled_revision?: WorkRevision | null,
	/**  Closed page outcome, independent of explanatory text. Absent on older steps. */
	outcome?: WorkPageOutcomeV1 | null,
	/**  A wall verified clear before the same retained task resumed. */
	resolved_handoff?: WorkPageHandoffKindV1 | null,
	/**  Sticky model-image disclosure provenance, absent on older/text-only steps. */
	image_disclosure?: WorkPageImageDisclosureV1 | null,
	/**  Wall time from the step's resource launch to its settled outcome. */
	wall_millis: number,
	/**  Typed decision calls answered by the recommended backend. */
	decision_calls: number,
	/**  Typed decision calls answered by the LLM emulation. */
	emulation_calls: number,
	/**  Page-model calls, including focused generation and extraction. */
	planner_calls: number,
	/**  Native actions the page agent started. */
	native_actions: number,
	/**  Tokens charged by settled provider calls. */
	model_tokens: number,
	/**  Micro-USD charged by settled provider calls. */
	cost_micro_usd: number,
	/**  How exactly `cost_micro_usd` is known. */
	cost_basis: WorkCostBasis,
};

/**
 *  Closed counts for one browser step: durations, call counts and exact
 *  accounting beside the conservative ceiling in `usage`. Never page, model
 *  or provider text.
 */
export type WorkStepMeasurementsV1_Serialize = {
	/**
	 *  Work revision assigned by Store when this page step settled. Never an
	 *  adapter clock or claim; absent on older or unfinished records.
	 */
	settled_revision?: WorkRevision | null,
	/**  Closed page outcome, independent of explanatory text. Absent on older steps. */
	outcome?: WorkPageOutcomeV1 | null,
	/**  A wall verified clear before the same retained task resumed. */
	resolved_handoff?: WorkPageHandoffKindV1 | null,
	/**  Sticky model-image disclosure provenance, absent on older/text-only steps. */
	image_disclosure?: WorkPageImageDisclosureV1 | null,
	/**  Wall time from the step's resource launch to its settled outcome. */
	wall_millis: number,
	/**  Typed decision calls answered by the recommended backend. */
	decision_calls: number,
	/**  Typed decision calls answered by the LLM emulation. */
	emulation_calls: number,
	/**  Page-model calls, including focused generation and extraction. */
	planner_calls: number,
	/**  Native actions the page agent started. */
	native_actions: number,
	/**  Tokens charged by settled provider calls. */
	model_tokens: number,
	/**  Micro-USD charged by settled provider calls. */
	cost_micro_usd: number,
	/**  How exactly `cost_micro_usd` is known. */
	cost_basis: WorkCostBasis,
};

export type WorkStepStatus = "running" | "succeeded" | "failed" | "cancelled" | "outcome_unknown";

/**  A named thing the work is about: a library, a listing, a flight, a concept. */
export type WorkSubject = WorkSubject_Serialize | WorkSubject_Deserialize;

/**  Where a subject stands, in its service's word, and the tone it reads in. */
export type WorkSubjectStateV1 = {
	label: string,
	tone: WorkSubjectToneV1,
};

export type WorkSubjectToneV1 =
/**  Open, in progress, unread. */
"active" |
/**  Merged, resolved, done. */
"done" |
/**  Waiting on review, on hold, draft. */
"waiting" |
/**  Failing, blocked, changes requested. */
"blocked" |
/**  Closed without being done, archived. */
"closed";

/**  A named thing the work is about: a library, a listing, a flight, a concept. */
export type WorkSubject_Deserialize = {
	name: string,
	descriptor?: string | null,
	/**  Descriptive link only; never a navigation grant. */
	homepage?: string | null,
	/**
	 *  Public HTTPS image URLs from cited sources that depict the subject.
	 *  Candidates only: Rust fetches, bounds, decodes, and stores an admitted
	 *  copy with provenance before anything renders.
	 */
	image_candidates?: string[],
	/**  Host-only immutable native record image; never supplied by model tools. */
	record_media?: WorkRecordMediaRefV1 | null,
	/**  Host withheld remote fallback for privacy/account authority; not an enrichment failure. */
	media_attempted?: boolean,
};

/**  A named thing the work is about: a library, a listing, a flight, a concept. */
export type WorkSubject_Serialize = {
	name: string,
	descriptor?: string | null,
	/**  Descriptive link only; never a navigation grant. */
	homepage?: string | null,
	/**
	 *  Public HTTPS image URLs from cited sources that depict the subject.
	 *  Candidates only: Rust fetches, bounds, decodes, and stores an admitted
	 *  copy with provenance before anything renders.
	 */
	image_candidates?: string[],
	/**  Host-only immutable native record image; never supplied by model tools. */
	record_media?: WorkRecordMediaRefV1 | null,
	/**  Host withheld remote fallback for privacy/account authority; not an enrichment failure. */
	media_attempted?: boolean,
};

export type WorkSummary = {
	lifecycle: WorkLifecycle,
	schema_version: number,
	id: WorkId,
	revision: WorkRevision,
	status: WorkAuthoringStatus,
	objective: string,
};

export type WorkUsage = {
	model_tokens: number,
	cost_micro_usd: number,
	operations: number,
	accounting: WorkUsageAccounting,
};

export type WorkUsageAccounting = "exact" | "conservative_reservation";

export type WorkUserEdit = { kind: "set_objective"; objective: string } | { kind: "open_question"; prompt: string; options: string[] } | { kind: "answer_question"; id: WorkQuestionId; answer: string } | { kind: "dismiss_question"; id: WorkQuestionId } | { kind: "replace_draft"; proposal: WorkPlanProposal } | { kind: "archive" } | { kind: "restore" } | { kind: "compact_history" };

export type ZoomCall =
/**  The sites the profile zoomed, by name. */
{ kind: "levels" } |
/**  Shows one site at the default zoom again. */
{ kind: "forget"; site: string } | { kind: "forget_all" };

export type ZoomError = "invalid" | "unavailable" | "capacity";

export type ZoomLevelView = {
	site: string,
	percent: number,
};

export type ZoomResponse =
/**  The profile's levels after the call, by site. */
{ kind: "levels"; levels: ZoomLevelView[] } | { kind: "error"; error: ZoomError };

/* Tauri Specta runtime */
async function typedError<T, E>(result: Promise<T>): Promise<{ status: "ok"; data: T } | { status: "error"; error: E }> {
    try {
        return { status: "ok", data: await result };
    } catch (e) {
        if (e instanceof Error) throw e;
        return { status: "error", error: e as any };
    }
}

type EventEmit<T> = [T] extends [null] ? () => Promise<void> : (payload: T) => Promise<void>;

function makeEvent<T>(name: string, serialize?: (payload: T) => unknown, deserialize?: (payload: any) => T) {
    const mapEvent = (cb: __TAURI_EVENT.EventCallback<T>) => (event: __TAURI_EVENT.Event<any>) => cb({ ...event, payload: deserialize ? deserialize(event.payload) : event.payload });
    const mapPayload = (payload: T) => serialize ? serialize(payload) : payload;

    const base = {
        listen: (cb: __TAURI_EVENT.EventCallback<T>) => __TAURI_EVENT.listen(name, mapEvent(cb)),
        once: (cb: __TAURI_EVENT.EventCallback<T>) => __TAURI_EVENT.once(name, mapEvent(cb)),
        emit: ((payload: T) => __TAURI_EVENT.emit(name, mapPayload(payload)) as unknown) as EventEmit<T>
    };

    const fn = (target: import("@tauri-apps/api/webview").Webview | import("@tauri-apps/api/window").Window) => ({
        listen: (cb: __TAURI_EVENT.EventCallback<T>) => target.listen(name, mapEvent(cb)),
        once: (cb: __TAURI_EVENT.EventCallback<T>) => target.once(name, mapEvent(cb)),
        emit: ((payload: T) => target.emit(name, mapPayload(payload)) as unknown) as EventEmit<T>
    });

    return Object.assign(fn, base);
}

