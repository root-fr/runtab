import type { SyncStatus } from "@/api/types";
import { SyncBanner } from "./SyncBanner";
import { SeeWhatSyncsDrawer } from "./SeeWhatSyncsDrawer";
import { ProjectReview } from "./ProjectReview";
import { SettingsDrawer } from "./SettingsDrawer";

export interface OverlayState {
  settings: boolean;
  seeWhat: boolean;
  review: boolean;
  bannerVisible: boolean;
}

interface OverlaysProps {
  sync: SyncStatus | undefined;
  state: OverlayState;
  set: (patch: Partial<OverlayState>) => void;
  onSaved: () => void;
  onEnableSync: () => void;
  onReviewSaved: () => void;
}

// All non-modal / drawer surfaces in one place so the dashboard body stays a
// clean layout. Each is independently controlled.
export function Overlays({
  sync, state, set, onSaved, onEnableSync, onReviewSaved,
}: OverlaysProps) {
  return (
    <>
      {state.bannerVisible && (
        <SyncBanner
          onSeeWhatSyncs={() => set({ seeWhat: true })}
          onEnableSync={onEnableSync}
          onDismiss={() => set({ bannerVisible: false })}
        />
      )}
      <SeeWhatSyncsDrawer open={state.seeWhat} onOpenChange={(v) => set({ seeWhat: v })} />
      <ProjectReview
        open={state.review}
        onOpenChange={(v) => set({ review: v })}
        onSaved={onReviewSaved}
      />
      <SettingsDrawer
        open={state.settings}
        onOpenChange={(v) => set({ settings: v })}
        sync={sync}
        onSaved={onSaved}
        onEnableSync={onEnableSync}
      />
    </>
  );
}
