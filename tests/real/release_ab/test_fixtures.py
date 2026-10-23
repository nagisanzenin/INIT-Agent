import unittest

from fixtures import FAMILIES, PHASES, LEARN_RETENTION_INSTRUCTION, build, hashes, registry
from harness import plan


class FixtureRegistrationTests(unittest.TestCase):
    def test_future_work_instruction_is_learn_only_and_keeps_data_and_gold(self):
        seed=2026102301
        fixtures=registry(seed)
        self.assertEqual(fixtures,registry(seed))
        for i,family in enumerate(FAMILIES):
            for j,phase in enumerate(PHASES):
                original=build(family,seed+100*i+j)
                registered=fixtures[family][phase]
                expected_prompt=original['prompt']+(LEARN_RETENTION_INSTRUCTION if phase=='learn' else '')
                self.assertEqual(registered['prompt'],expected_prompt)
                self.assertEqual(registered,{**original,'prompt':expected_prompt})

    def test_both_arms_use_identical_registered_phase_prompts(self):
        fixtures=registry(); bound=hashes(fixtures); slots=plan()['tasks']
        for family in FAMILIES:
            for phase in PHASES:
                prompts={task['arm']:fixtures[task['family']][task['phase']]['prompt']
                         for task in slots if task['family']==family and task['phase']==phase}
                self.assertEqual(set(prompts),{'A','B'})
                self.assertEqual(prompts['A'],prompts['B'])
                self.assertEqual(len(bound[family][phase]['prompt_sha']),64)

    def test_retention_instruction_leaves_evolution_optional_and_has_no_reward(self):
        self.assertIn('available and worthwhile',LEARN_RETENTION_INSTRUCTION)
        self.assertIn('normal admission process',LEARN_RETENTION_INSTRUCTION)
        self.assertIn('unavailable or refused',LEARN_RETENTION_INSTRUCTION)
        for hidden in ('score','reward','expected=','answer.json','validation','navigation'):
            self.assertNotIn(hidden,LEARN_RETENTION_INSTRUCTION)


if __name__=='__main__': unittest.main()
