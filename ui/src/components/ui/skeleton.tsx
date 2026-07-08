import { cn } from "@/lib/utils";

// Loading placeholder. Deliberately a soft pulse, never a spinner, per the
// craft rules: the panel keeps its shape while data arrives.
export function Skeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("animate-pulse rounded-md bg-muted/70", className)} {...props} />;
}
