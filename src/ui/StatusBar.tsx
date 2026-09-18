import { hex, locked, blockCompare, tapeCompare, audioMode, status, toggleLock, fmtTime, blockNo, tapes } from '../state/store';
import { playing, playingSide, playingBlock, playPos, stopPlayback } from '../state/player';
import { Icon } from './icons';

const BLOCK_MODES = ['data', 'data+timings', 'data+timings+pauses'] as const;
const TAPE_MODES = ['datablocks', 'ignore-metadata', 'all'] as const;
const TAPE_MODE_LABEL: Record<string, string> = { datablocks: 'data blocks only', 'ignore-metadata': 'ignore metadata', all: 'all blocks' };

function PlayProgress() {
  const { elapsed, total } = playPos.value;
  const side = playingSide.value;
  const i = playingBlock.value;
  const block = side !== null && i >= 0 ? tapes[side].value.blocks[i] : undefined;
  const pct = total > 0 ? (elapsed / total) * 100 : 0;
  return (
    <div class="cell grow playprogress" title="Click to stop" onClick={() => stopPlayback()}>
      <Icon name="play" size={13} />
      <span class="time">{fmtTime(elapsed)} / {fmtTime(total)}</span>
      <span class="bar"><span class="fill" style={{ width: `${pct}%` }} /></span>
      {block && <span class="block">Block {blockNo(i)}</span>}
    </div>
  );
}

export function StatusBar() {
  return (
    <div class="statusbar">
      <div class={'cell' + (hex.value ? ' on' : '')} title="Number base for all numbers" onClick={() => (hex.value = !hex.value)}>
        <Icon name="hash" size={13} /><b>{hex.value ? 'Hex' : 'Dec'}</b>
      </div>
      <div class="cell" title="How two blocks are compared" onClick={() => (blockCompare.value = BLOCK_MODES[(BLOCK_MODES.indexOf(blockCompare.value) + 1) % 3])}>
        Block compare <b>{blockCompare.value.replace(/\+/g, ' + ')}</b>
      </div>
      <div class="cell" title="Which blocks take part in tape compare" onClick={() => (tapeCompare.value = TAPE_MODES[(TAPE_MODES.indexOf(tapeCompare.value) + 1) % 3])}>
        <Icon name="compare" size={13} />Tape compare <b>{TAPE_MODE_LABEL[tapeCompare.value]}</b>
      </div>
      <div class={'cell' + (locked.value ? ' on' : '')} title={locked.value ? 'Locked: click to allow editing' : 'Unlocked: click to prevent edits'} onClick={toggleLock}>
        <Icon name={locked.value ? 'lock' : 'unlock'} size={13} />{locked.value ? 'Locked' : 'Unlocked'}
      </div>
      <div class="cell" title="Waveform used for playback and WAV export" onClick={() => (audioMode.value = audioMode.value === 'mic' ? 'square' : 'mic')}>
        <Icon name="wave" size={13} />{audioMode.value === 'mic' ? 'MIC emulation' : 'Square wave'}
      </div>
      {playing.value ? <PlayProgress /> : <div class="cell grow">{status.value}</div>}
    </div>
  );
}
