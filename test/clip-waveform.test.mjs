import test from 'node:test';
import assert from 'node:assert/strict';
import { waveformPeaks, waveformWindow, WaveformReview } from '../public/clip-waveform.js';
const buffer=(channels,sampleRate=8000)=>({length:channels[0].length,numberOfChannels:channels.length,sampleRate,getChannelData:i=>channels[i]});
test('waveform preserves stereo transients, partial final bins and the decoded originals',()=>{
  const channels=[new Float32Array([0,.2,-.8,.1,1]),new Float32Array([-.9,0,.6,-.2,-1])],original=channels.map(a=>Array.from(a));
  const summary=waveformPeaks(buffer(channels),2);
  assert.equal(summary.length,5);assert.equal(summary.durationSeconds,5/8000);assert.equal(summary.numberOfChannels,2);
  assert.ok(Math.abs(summary.peaks[0].min+.9)<1e-6);assert.ok(Math.abs(summary.peaks[0].max-.2)<1e-6);assert.deepEqual(summary.peaks[1],{min:-1,max:1});assert.deepEqual(channels.map(a=>Array.from(a)),original);
  const sparse=waveformPeaks(buffer([new Float32Array([0,2,-2])]),256);assert.equal(sparse.peaks.length,3);assert.deepEqual(sparse.peaks,[{min:0,max:0},{min:0,max:2},{min:-2,max:0}]);
});
test('waveform rejects invalid dimensions, oversized decode, incomplete channels and non-finite samples',()=>{
  const valid=buffer([new Float32Array([0,1])]);
  for(const patch of [{sampleRate:0},{length:0},{length:8000*121},{sampleRate:384000,length:384000*120},{numberOfChannels:33},{numberOfChannels:0},{getChannelData:()=>new Float32Array(1)},{getChannelData:()=>new Float32Array([0,NaN])}])assert.throws(()=>waveformPeaks({...valid,...patch}));
  for(const count of [0,-1,1025,1.5,NaN])assert.throws(()=>waveformPeaks(valid,count));
});
test('review maps exact source seconds and fades while exposing decoded-duration differences',()=>{
  const audio={durationSeconds:4,trimStartSeconds:1,trimEndSeconds:3,fadeInSeconds:.25,fadeOutSeconds:.5},original=structuredClone(audio),view=waveformWindow(audio,{durationSeconds:2});
  assert.equal(view.axisSeconds,4);assert.equal(view.decodedSeconds,2);assert.equal(view.sourceIn,1);assert.equal(view.sourceOut,3);assert.equal(view.audibleSeconds,1);
  assert.deepEqual(view.fadePoints,[{seconds:1,level:0},{seconds:1.25,level:1},{seconds:1.5,level:1},{seconds:2,level:0}]);assert.deepEqual(audio,original);
  const shorter=waveformWindow({...audio,trimStartSeconds:2.5,fadeInSeconds:.1,fadeOutSeconds:.1},{durationSeconds:2});assert.equal(shorter.audibleSeconds,0);assert.deepEqual(shorter.fadePoints,[]);
  assert.equal(waveformWindow(audio,{durationSeconds:5}).axisSeconds,5);assert.throws(()=>waveformWindow(audio,{durationSeconds:NaN}));
});
test('a late decode cannot replace a new track or a closed review',async()=>{
  const pending=new Map(),events=[],review=new WaveformReview({load:id=>new Promise(resolve=>pending.set(id,resolve)),onChange:()=>events.push(review.key)}),data=buffer([new Float32Array([0,.5])]);
  review.select('project-a/track-a','asset-a');const first=review.open();assert.equal(review.loading,true);
  review.select('project-b/track-b','asset-b');const second=review.open();pending.get('asset-a')(data);await first;assert.equal(review.loading,true);assert.equal(review.data,undefined);assert.equal(events.length,2);
  pending.get('asset-b')(data);await second;assert.equal(review.loading,false);assert.equal(review.data.length,2);assert.equal(events.at(-1),'project-b/track-b');
  review.select('project-b/track-c','asset-c');const third=review.open();review.close();const count=events.length;pending.get('asset-c')(data);await third;assert.equal(review.expanded,false);assert.equal(review.data,null);assert.equal(events.length,count);
});
test('failed waveform loads can retry and the bounded cache reopens without decoding',async()=>{
  let calls=0;const review=new WaveformReview({load:async()=>{if(++calls===1)throw Error('Fixture decode failed');return buffer([new Float32Array([0])]);}});
  review.select('project/track','asset-0');await review.open();assert.match(review.error,/decode failed/);assert.equal(review.loading,false);
  review.close();await review.open();assert.equal(review.error,null);assert.equal(review.data.length,1);review.close();await review.open();assert.equal(calls,2);
  for(let i=1;i<=9;i++){review.select('project/track-'+i,'asset-'+i);await review.open();}assert.equal(review.cache.size,8);assert.equal(review.cache.has('asset-0'),false);
});
