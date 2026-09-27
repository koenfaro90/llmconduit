import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/react';
import { useState } from 'react';
import { FacetSelect } from './FacetSelect';
import { emptyFacet } from './facetModel';

afterEach(cleanup);

it('keeps options in place when include or exclude changes', () => {
  function Fixture() {
    const [value, setValue] = useState(emptyFacet());
    return <FacetSelect label="Model" options={['Zulu', 'Bravo', 'Alpha']} value={value} onChange={setValue} />;
  }
  const view = render(<Fixture />);
  fireEvent.click(view.getByRole('button', { name: 'Model' }));
  const order = () => view.getAllByRole('checkbox').map((checkbox) => checkbox.getAttribute('aria-label'));
  const initial = order();
  expect(initial).toEqual([
    'Include Model: Alpha', 'Exclude Model: Alpha',
    'Include Model: Bravo', 'Exclude Model: Bravo',
    'Include Model: Zulu', 'Exclude Model: Zulu',
  ]);
  fireEvent.click(view.getByRole('checkbox', { name: 'Include Model: Zulu' }));
  expect(order()).toEqual(initial);
  fireEvent.click(view.getByRole('checkbox', { name: 'Exclude Model: Bravo' }));
  expect(order()).toEqual(initial);
});
