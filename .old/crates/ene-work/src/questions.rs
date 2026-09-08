use crate::types::{CombinedQuestionTurn, OpenQuestion};
use ene_session::DelegationId;
use std::fmt::Write;

/// Merge multiple child questions into one parent ask-user turn.
#[must_use]
pub fn combine_questions(questions: &[OpenQuestion]) -> CombinedQuestionTurn {
    if questions.is_empty() {
        return CombinedQuestionTurn {
            speech: String::new(),
            questions: Vec::new(),
        };
    }
    if questions.len() == 1 {
        return CombinedQuestionTurn {
            speech: questions[0].prompt.clone(),
            questions: questions.to_vec(),
        };
    }
    let mut speech = String::from("A few quick questions:\n");
    for (index, question) in questions.iter().enumerate() {
        writeln!(speech, "{}. {}", index + 1, question.prompt).ok();
    }
    CombinedQuestionTurn {
        speech,
        questions: questions.to_vec(),
    }
}

/// Route combined answers back to the delegations that asked.
#[must_use]
pub fn route_combined_answers(
    turn: &CombinedQuestionTurn,
    answers: &[String],
) -> Vec<(DelegationId, String)> {
    turn.questions
        .iter()
        .zip(answers.iter())
        .map(|(question, answer)| (question.delegation_id, answer.clone()))
        .collect()
}
