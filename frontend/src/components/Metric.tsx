export function Metric(props: { label: string; value: string; hot?: boolean }) {
  return (
    <div class="metric" classList={{ hot: props.hot }}>
      <span>{props.label}</span>
      <strong>{props.value}</strong>
    </div>
  );
}
