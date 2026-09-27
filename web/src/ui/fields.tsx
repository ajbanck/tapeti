import { useState, useEffect } from 'preact/hooks';
import { ComponentChildren } from 'preact';
import { fmtNum, parseNum, hex } from '../state/store';

interface NumInputProps {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  disabled?: boolean;
  width?: number;
  title?: string;
  /** The switch of the screen the field is on; the main window's by default. */
  hex?: boolean;
}

/** Numeric text input honouring its screen's Dec/Hex switch; keeps invalid text locally. */
export function NumInput({ value, onChange, min = 0, max = 0xffffffff, disabled, width, title, hex: ownHex }: NumInputProps) {
  const h = ownHex ?? hex.value;
  const [text, setText] = useState(fmtNum(value, h));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    setText(fmtNum(value, h));
    setBad(false);
  }, [value, h]);
  return (
    <input
      type="text"
      value={text}
      title={title}
      disabled={disabled}
      class={bad ? 'invalid' : ''}
      style={width ? { width } : undefined}
      onInput={(e) => {
        const s = (e.target as HTMLInputElement).value;
        setText(s);
        const v = parseNum(s, h);
        if (Number.isNaN(v) || v < min || v > max) setBad(true);
        else {
          setBad(false);
          if (v !== value) onChange(v);
        }
      }}
      onBlur={() => {
        if (bad) {
          setText(fmtNum(value, h));
          setBad(false);
        }
      }}
    />
  );
}

export function TextInput({ value, onChange, maxLength, width, disabled }: { value: string; onChange: (s: string) => void; maxLength?: number; width?: number | string; disabled?: boolean }) {
  return (
    <input
      type="text"
      value={value}
      maxLength={maxLength}
      disabled={disabled}
      style={width ? { width } : undefined}
      onInput={(e) => onChange((e.target as HTMLInputElement).value)}
    />
  );
}

export function Field({ label, children }: { label: string; children: ComponentChildren }) {
  return (
    <>
      <label>{label}</label>
      <div>{children}</div>
    </>
  );
}

export function Check({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <label style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange((e.target as HTMLInputElement).checked)} />
      {label}
    </label>
  );
}
