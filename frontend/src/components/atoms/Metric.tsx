import { Show } from "solid-js";
import { Dynamic } from "solid-js/web";
import type { ValidComponent } from "solid-js";

export function Metric(props: { label: string; value: string; hot?: boolean; icon?: ValidComponent }) {
  return (
    <div class="min-w-[54px] grid gap-[1px] content-center px-2 border-r border-soft-line last:border-0">
      <span class="flex items-center gap-1 text-[#93a1af] text-[0.64rem] leading-none">
        <Show when={props.icon}>
          <Dynamic component={props.icon} size={11} strokeWidth={2.5} class="opacity-80" />
        </Show>
        {props.label}
      </span>
      <strong 
        class="overflow-hidden text-ellipsis whitespace-nowrap font-heading font-semibold text-[0.8rem] leading-[1.1]"
        classList={{
          "text-green": props.hot,
          "text-[#d9e2ec]": !props.hot
        }}
      >
        {props.value}
      </strong>
    </div>
  );
}
