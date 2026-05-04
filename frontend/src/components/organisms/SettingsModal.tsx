import { Save, X } from "lucide-solid";
import { createSignal, Show } from "solid-js";
import { saveSettings } from "../../api";
import type { DashboardStatus } from "../../types";

export function SettingsModal(props: {
  current: DashboardStatus;
  onClose: () => void;
  onRefresh: () => void;
}) {
  const [draftLive, setDraftLive] = createSignal(!props.current.dry_run && props.current.allow_live_buys);
  const [draftLiveSells, setDraftLiveSells] = createSignal(props.current.allow_live_sells);
  const [draftPrivateKey, setDraftPrivateKey] = createSignal("");
  const [draftFunder, setDraftFunder] = createSignal(props.current.funder_address ?? "");
  const [draftSignatureType, setDraftSignatureType] = createSignal(
    props.current.signature_type === null || props.current.signature_type === undefined ? "" : String(props.current.signature_type)
  );
  const [draftMaxOrder, setDraftMaxOrder] = createSignal(props.current.live_max_order_usd || 1);
  const [draftSnipeMax, setDraftSnipeMax] = createSignal(props.current.snipe_max_position_usd || 5);
  const [settingsMessage, setSettingsMessage] = createSignal("");
  const [savingSettings, setSavingSettings] = createSignal(false);

  async function submitSettings() {
    if (draftLive() && !props.current.wallet_configured && !draftPrivateKey().trim()) {
      setSettingsMessage("Live mode needs a private key.");
      return;
    }
    const signatureType = draftSignatureType().trim() === "" ? null : Number(draftSignatureType());
    setSavingSettings(true);
    setSettingsMessage("Saving settings...");
    try {
      const result = await saveSettings({
        dry_run: !draftLive(),
        allow_live_buys: draftLive(),
        allow_live_sells: draftLiveSells(),
        live_max_order_usd: draftMaxOrder(),
        snipe_max_position_usd: draftSnipeMax(),
        funder_address: draftFunder(),
        signature_type: signatureType,
        private_key: draftPrivateKey().trim() ? draftPrivateKey() : null,
      });
      if (!result.ok) {
        setSettingsMessage(result.error ?? "Settings were rejected.");
        return;
      }
      setSettingsMessage("Settings saved.");
      setDraftPrivateKey("");
      props.onRefresh();
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setSavingSettings(false);
    }
  }

  return (
    <div class="fixed inset-0 z-50 flex justify-end pt-[58px] px-7 pb-0 bg-[rgba(0,0,0,0.16)] backdrop-blur-sm" onClick={props.onClose}>
      <section class="w-[min(360px,calc(100vw-28px))] h-max grid gap-3 p-3.5 border border-soft-line rounded-[10px] bg-[#12191f] shadow-[0_18px_50px_rgba(0,0,0,0.35)]" onClick={(event) => event.stopPropagation()}>
        <div class="flex items-center justify-between gap-3">
          <strong class="text-[0.96rem]">Settings</strong>
          <button type="button" class="inline-flex items-center gap-1.5 text-[#9aa9b7] border-0 bg-transparent cursor-pointer text-[0.74rem] hover:text-white" onClick={props.onClose} title="Close settings">
            <X size={15} class="flex-none" />
            <span>Close</span>
          </button>
        </div>
        <label class="grid grid-cols-[1fr_auto] items-center justify-between gap-3">
          <span>
            <strong class="block text-[#f4f7fb] text-[0.82rem]">Trading mode (Buy)</strong>
            <small class="text-[#91a0af] text-[0.7rem] font-extrabold">{draftLive() ? "Live orders enabled" : "Paper orders only"}</small>
          </span>
          <input type="checkbox" checked={draftLive()} onInput={(event) => setDraftLive(event.currentTarget.checked)} />
        </label>
        <label class="grid grid-cols-[1fr_auto] items-center justify-between gap-3 border-t border-soft-line pt-2">
          <span>
            <strong class="block text-[#f4f7fb] text-[0.82rem]">Trading mode (Sell)</strong>
            <small class="text-[#91a0af] text-[0.7rem] font-extrabold">{draftLiveSells() ? "Early exits enabled" : "Hold to resolution"}</small>
          </span>
          <input type="checkbox" checked={draftLiveSells()} onInput={(event) => setDraftLiveSells(event.currentTarget.checked)} />
        </label>
        <label class="grid gap-1.5">
          <span class="text-[#91a0af] text-[0.7rem] font-extrabold">Polymarket private key</span>
          <input
            class="w-full h-[34px] text-[#f4f7fb] border border-soft-line rounded-md bg-[#19222a] px-2.5 font-inherit text-[0.76rem] outline-none focus:border-blue"
            type="password"
            autocomplete="off"
            placeholder={props.current.wallet_configured ? "Configured. Enter new key to replace." : "Required for wallet and live orders"}
            value={draftPrivateKey()}
            onInput={(event) => setDraftPrivateKey(event.currentTarget.value)}
          />
        </label>
        <label class="grid gap-1.5">
          <span class="text-[#91a0af] text-[0.7rem] font-extrabold">Funder address</span>
          <input class="w-full h-[34px] text-[#f4f7fb] border border-soft-line rounded-md bg-[#19222a] px-2.5 font-inherit text-[0.76rem] outline-none focus:border-blue" value={draftFunder()} onInput={(event) => setDraftFunder(event.currentTarget.value)} placeholder="Optional proxy/funder address" />
        </label>
        <label class="grid gap-1.5">
          <span class="text-[#91a0af] text-[0.7rem] font-extrabold">Signature type</span>
          <select class="w-full h-[34px] text-[#f4f7fb] border border-soft-line rounded-md bg-[#19222a] px-2.5 font-inherit text-[0.76rem] outline-none focus:border-blue" value={draftSignatureType()} onInput={(event) => setDraftSignatureType(event.currentTarget.value)}>
            <option value="">EOA / default</option>
            <option value="1">Proxy</option>
            <option value="2">Gnosis Safe</option>
            <option value="3">Poly 1271</option>
          </select>
        </label>
        <div class="grid grid-cols-2 gap-3">
          <label class="grid gap-1.5">
            <span class="text-[#91a0af] text-[0.7rem] font-extrabold">Max live order ($)</span>
            <input class="w-full h-[34px] text-[#f4f7fb] border border-soft-line rounded-md bg-[#19222a] px-2.5 font-inherit text-[0.76rem] outline-none focus:border-blue" type="number" min="1" step="0.5" value={draftMaxOrder()} onInput={(event) => setDraftMaxOrder(Number(event.currentTarget.value || 0))} />
          </label>
          <label class="grid gap-1.5">
            <span class="text-[#91a0af] text-[0.7rem] font-extrabold">Snipe target ($)</span>
            <input class="w-full h-[34px] text-[#f4f7fb] border border-soft-line rounded-md bg-[#19222a] px-2.5 font-inherit text-[0.76rem] outline-none focus:border-blue" type="number" min="1" step="0.5" value={draftSnipeMax()} onInput={(event) => setDraftSnipeMax(Number(event.currentTarget.value || 0))} />
          </label>
        </div>
        <button type="button" class="min-h-[38px] inline-flex items-center justify-center gap-[7px] text-white border-0 rounded-md bg-blue cursor-pointer text-[0.78rem] font-black hover:brightness-110 transition-all duration-200 disabled:opacity-50 disabled:cursor-not-allowed" disabled={savingSettings()} onClick={submitSettings}>
          <Save size={15} class="flex-none" />
          <span>{savingSettings() ? "Saving..." : "Save settings"}</span>
        </button>
        <small class="text-[#8494a4] text-[0.68rem] leading-[1.35]">{settingsMessage() || "Settings are saved to .env and persist across restarts. Private key stays local and is never returned in status."}</small>
      </section>
    </div>
  );
}
