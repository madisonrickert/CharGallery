import { render } from '@testing-library/react';
import { ScreenSaver } from './ScreenSaver';

beforeEach(() => {
  // jsdom doesn't implement HTMLMediaElement.play()
  HTMLMediaElement.prototype.play = vi.fn().mockResolvedValue(undefined);
  HTMLMediaElement.prototype.pause = vi.fn();
});

describe('ScreenSaver', () => {
  it('renders with visible class when shouldShow is true', () => {
    const { container } = render(<ScreenSaver shouldShow={true} />);
    expect(container.querySelector('.screen-saver')).toHaveClass('visible');
  });

  it('does not have visible class when shouldShow is false', () => {
    const { container } = render(<ScreenSaver shouldShow={false} />);
    expect(container.querySelector('.screen-saver')).not.toHaveClass('visible');
  });

  it('renders a video element', () => {
    const { container } = render(<ScreenSaver shouldShow={false} />);
    expect(container.querySelector('video')).toBeInTheDocument();
  });

  it('video does not have autoplay attribute', () => {
    const { container } = render(<ScreenSaver shouldShow={false} />);
    expect(container.querySelector('video')?.autoplay).toBe(false);
  });
});
