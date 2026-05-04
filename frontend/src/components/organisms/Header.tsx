import { Bot, Clock3, DollarSign, Layers, ListOrdered, Settings, ShieldCheck, Wallet } from "lucide-solid";
import { Metric } from "../atoms/Metric";

interface WalletStatus {
  portfolio_value?: number | null;
  cash?: number | null;
  positions_count: number;
  open_orders?: any[];
  allowance?: number | null;
  error?: string | null;
}

export function Header(props: {
  live: boolean;
  wallet: WalletStatus;
  clockZone: "local" | "utc";
  setClockZone: (z: "local" | "utc") => void;
  displayClock: string;
  onOpenSettings: () => void;
  compactUsd: (value?: number | null) => string;
}) {
  return (
    <header class="h-[60px] grid grid-cols-1 max-[1000px]:grid-cols-1 min-[1000px]:grid-cols-[minmax(180px,260px)_minmax(340px,auto)] items-center justify-center gap-3 px-7 py-2 border-b border-soft-line bg-bar backdrop-blur-md sticky top-0 z-50 shadow-[0_4px_30px_rgba(0,0,0,0.3)]">
      <div class="flex items-center gap-2 min-w-0 max-[720px]:justify-start">
        <span class="w-[30px] h-[30px] grid place-items-center text-white border border-soft-line rounded-md bg-[#202a33] text-[0.72rem] font-black leading-none flex-none">
          <Bot size={16} strokeWidth={2.4} />
        </span>
        <div>
          <strong class="block font-heading text-[1.05rem] font-semibold tracking-normal text-[#f4f7fb]">5m Snipe Bot</strong>
          <small class="block text-[0.78rem] text-[#818cf8]">{props.live ? "live trading enabled" : "paper trading"}</small>
        </div>
      </div>
      <div class="flex items-center justify-end gap-2.5 min-w-0 max-[1000px]:justify-start max-[720px]:flex-wrap">
        <Metric label="Portfolio" value={props.compactUsd(props.wallet.portfolio_value)} hot={!props.wallet.error} icon={Wallet} />
        <Metric label="Cash" value={props.compactUsd(props.wallet.cash)} hot={!props.wallet.error} icon={DollarSign} />
        <Metric label="Positions" value={String(props.wallet.positions_count)} icon={Layers} />
        <Metric label="Open Orders" value={String(props.wallet.open_orders?.length ?? 0)} icon={ListOrdered} />
        <Metric label="Allowance" value={props.compactUsd(props.wallet.allowance)} hot={!props.wallet.error} icon={ShieldCheck} />
        <button
          type="button"
          class="h-[34px] min-w-[86px] grid content-center gap-px px-2.5 text-[#dce6ef] border border-soft-line rounded-md bg-[#172028] text-left cursor-pointer transition-all duration-200 hover:brightness-110"
          onClick={() => props.setClockZone(props.clockZone === "local" ? "utc" : "local")}
          title="Switch local/UTC time"
        >
          <span class="text-[#93a1af] text-[0.58rem] leading-none">{props.clockZone === "local" ? "Local" : "UTC"}</span>
          <strong class="inline-flex items-center gap-1.5 text-[#f4f7fb] text-[0.76rem] leading-[1.05]">
            <Clock3 size={13} class="flex-none" />
            {props.displayClock}
          </strong>
        </button>
        <button type="button" class="h-[30px] inline-flex items-center gap-1.5 px-3 text-[#dce6ef] border border-soft-line rounded-md bg-[#202a33] text-[0.74rem] font-extrabold cursor-pointer transition-all duration-200 hover:brightness-110" onClick={props.onOpenSettings} title="Open settings">
          <Settings size={15} class="flex-none" />
          <span>Settings</span>
        </button>
      </div>
    </header>
  );
}
