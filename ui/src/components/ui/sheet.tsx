import * as React from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";

// Right-side drawer built on Radix Dialog: focus trap, escape-to-close, and
// aria wiring come for free. Used for settings, "see what syncs", and the
// pre-sync project review.

export const Sheet = Dialog.Root;
export const SheetTrigger = Dialog.Trigger;
export const SheetClose = Dialog.Close;

interface SheetContentProps {
  title: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
  widthClass?: string;
}

export function SheetContent({
  title,
  description,
  children,
  className,
  widthClass = "max-w-md",
}: SheetContentProps) {
  return (
    <Dialog.Portal>
      <Dialog.Overlay className="fixed inset-0 z-40 bg-black/60 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0" />
      <Dialog.Content
        className={cn(
          "fixed inset-y-0 right-0 z-50 flex w-full flex-col gap-4 border-l border-border bg-card p-6 shadow-xl outline-none transition-transform duration-150 data-[state=open]:animate-in data-[state=open]:slide-in-from-right data-[state=closed]:animate-out data-[state=closed]:slide-out-to-right",
          widthClass,
          className,
        )}
      >
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1">
            <Dialog.Title className="text-base font-semibold tracking-tight">{title}</Dialog.Title>
            {description ? (
              <Dialog.Description className="text-sm text-muted-foreground">
                {description}
              </Dialog.Description>
            ) : (
              <Dialog.Description className="sr-only">{title}</Dialog.Description>
            )}
          </div>
          <Dialog.Close
            aria-label="Close"
            className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
          >
            <X className="h-4 w-4" />
          </Dialog.Close>
        </div>
        <div className="flex-1 overflow-y-auto">{children}</div>
      </Dialog.Content>
    </Dialog.Portal>
  );
}
