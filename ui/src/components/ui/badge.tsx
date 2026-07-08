import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium transition-colors",
  {
    variants: {
      tone: {
        neutral: "border-border bg-secondary text-secondary-foreground",
        accent: "border-transparent bg-accent/10 text-brand-strong",
        ok: "border-transparent bg-emerald-50 text-emerald-700",
        warn: "border-transparent bg-amber-50 text-amber-700",
        danger: "border-transparent bg-red-50 text-red-700",
        muted: "border-border bg-transparent text-muted-foreground",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

export type BadgeProps = React.HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>;

export function Badge({ className, tone, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}
